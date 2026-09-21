package com.byteowlz.xlatch

import android.app.Application
import android.content.Intent
import androidx.compose.runtime.*
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

fun operation(op: String, action: String) =
    JSONObject().put("op", op).put("request", JSONObject().put("action", action))

data class Review(val server: Server, val kind: String, val payload: String)

class AppModel(application: Application) : AndroidViewModel(application) {
    val store = Store(application)
    private val api = Api(store)
    var servers by mutableStateOf<List<Server>>(emptyList())
        private set

    var selected by mutableStateOf<Server?>(null)
        private set

    var actions by mutableStateOf<List<JSONObject>>(emptyList())
        private set

    var jobs by mutableStateOf<List<JSONObject>>(emptyList())
        private set

    var outbox by mutableStateOf<List<JSONObject>>(emptyList())
        private set

    var enrollment by mutableStateOf<JSONObject?>(null)
        private set

    var catalog by mutableStateOf<JSONObject?>(null)
        private set

    var deviceStatus by mutableStateOf<JSONObject?>(null)
        private set

    var pending by mutableStateOf<List<String>>(emptyList())
        private set

    var disabled by mutableStateOf<Set<String>>(emptySet())
        private set

    var content by mutableStateOf<List<JSONObject>>(emptyList())
        private set

    var review by mutableStateOf<Review?>(null)
    var result by mutableStateOf<String?>(null)
    var error by mutableStateOf<String?>(null)
    var busy by mutableStateOf(false)
        private set

    init {
        task {
            servers = withContext(Dispatchers.IO) { store.servers() }
            selected = servers.firstOrNull()
            refreshData(true)
        }
    }

    private fun task(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        viewModelScope.launch {
            try {
                block()
            } catch (failure: Exception) {
                error = failure.message ?: "Operation failed"
            } finally {
                busy = false
            }
        }
    }

    private suspend fun refreshData(network: Boolean) {
        outbox = withContext(Dispatchers.IO) { store.all("outbox").map { JSONObject(it.second) } }
        val server = selected ?: return
        actions = withContext(Dispatchers.IO) { store.catalog(server) }
        disabled =
            withContext(Dispatchers.IO) {
                actions
                    .filter { !store.enabled(server, it) }
                    .map { it.getJSONObject("manifest").getString("id") }
                    .toSet()
            }
        if (!network) return
        enrollment =
            withContext(Dispatchers.IO) {
                api.rpc(server, operation("enrollment", "status")) as JSONObject
            }
        if (enrollment?.optString("device_status") != "active") {
            actions = emptyList()
            return
        }
        actions = withContext(Dispatchers.IO) { api.discover(server).objects() }
        disabled =
            withContext(Dispatchers.IO) {
                actions
                    .filter { !store.enabled(server, it) }
                    .map { it.getJSONObject("manifest").getString("id") }
                    .toSet()
            }
        deviceStatus = null
        catalog = null
        pending = emptyList()
        jobs =
            withContext(Dispatchers.IO) {
                (api.rpc(server, JSONObject().put("op", "jobs")) as JSONArray).objects()
            }
        if (enrollment?.optBoolean("is_approver") == true) {
            deviceStatus = withContext(Dispatchers.IO) {
                api.rpc(server, deviceOperation(JSONObject().put("action", "list"))) as JSONObject
            }

            catalog =
                withContext(Dispatchers.IO) {
                    api.rpc(server, operation("approval", "catalog")) as JSONObject
                }
            pending =
                withContext(Dispatchers.IO) {
                    (api.rpc(server, operation("enrollment", "pending")) as JSONArray).strings()
                }
        }
    }

    fun refresh() = task { refreshData(true) }

    fun refreshOutbox() = task { refreshData(false) }

    fun select(server: Server) = task {
        selected = server
        actions = emptyList()
        jobs = emptyList()
        enrollment = null
        deviceStatus = null
        catalog = null
        pending = emptyList()
        refreshData(true)
    }

    fun pair(code: String, name: String) = task {
        selected = withContext(Dispatchers.IO) { api.pair(code, name) }
        servers = withContext(Dispatchers.IO) { store.servers() }
        refreshData(true)
    }

    fun share(intent: Intent) {
        viewModelScope.launch {
            try {
                content = withContext(Dispatchers.IO) { readShare(getApplication(), intent) }
            } catch (failure: Exception) {
                error = failure.message
            }
        }
    }

    fun compose(text: String) {
        require(text.toByteArray().size <= 100000)
        content =
            listOf(
                JSONObject()
                    .put("text", text)
                    .put(
                        "mime_type",
                        if (text.trim().matches(Regex("https?://\\S+"))) "text/uri-list"
                        else "text/plain",
                    )
            )
    }

    fun clearContent() {
        content = emptyList()
    }

    fun queue(action: JSONObject) = task {
        val server = selected ?: error("Choose a server")
        val inputs = content.toList()
        require(inputs.isNotEmpty()) { "Share or write some content first" }
        require(inputs.all { accepts(action, it.getString("mime_type")) }) {
            "Action cannot accept this content"
        }
        withContext(Dispatchers.IO) {
            store.writableDatabase.beginTransaction()
            try {
                inputs.forEach { store.queue(server, action, it) }
                store.writableDatabase.setTransactionSuccessful()
            } finally {
                store.writableDatabase.endTransaction()
            }
            scheduleDelivery(getApplication())
        }
        content = emptyList()
        refreshData(false)
    }

    fun enable(action: JSONObject, on: Boolean) = task {
        selected?.let { server ->
            withContext(Dispatchers.IO) { store.enable(server, action, on) }
            refreshData(false)
        }
    }

    fun retry(item: JSONObject) = task {
        withContext(Dispatchers.IO) {
            require(item.optString("status") == "paused")
            item.put("status", "queued")
            item.remove("error")
            store.put("outbox", item.getString("id"), item.toString())
            scheduleDelivery(getApplication())
        }
        refreshData(false)
    }

    fun discard(item: JSONObject) = task {
        withContext(Dispatchers.IO) { store.remove("outbox", item.getString("id")) }
        refreshData(false)
    }

    fun job(id: String, cancel: Boolean = false) = task {
        val server = selected ?: return@task
        result =
            withContext(Dispatchers.IO) {
                (api.rpc(
                        server,
                        JSONObject().put("op", if (cancel) "cancel" else "job").put("id", id),
                    ) as JSONObject)
                    .toString(2)
            }
        refreshData(true)
    }

    fun prepare(action: JSONObject, devices: Set<String>) = task {
        val server = selected ?: error("Choose server")
        val request = operation("approval", "prepare")
        request
            .getJSONObject("request")
            .put("capability_id", action.getJSONObject("manifest").getString("id"))
            .put("revision", action.getString("revision"))
            .put("devices", JSONArray(devices.toList()))
        val payload = withContext(Dispatchers.IO) { api.rpc(server, request) as String }
        val parsed = JSONObject(payload)
        validateReview(parsed, server, "capability")
        require(
            parsed.getString("revision") == action.getString("revision") &&
                parsed.getJSONObject("manifest").getString("id") ==
                    action.getJSONObject("manifest").getString("id")
        )
        require(
            parsed.getJSONArray("devices").objects().map { it.getString("id") }.toSet() == devices
        )
        review = Review(server, "capability", payload)
    }

    private fun deviceOperation(request: JSONObject): JSONObject =
        operation("enrollment", "devices").also { it.getJSONObject("request").put("request", request) }

    fun reviewDevice(payload: String) {
        try {
            val server = selected ?: return
            val json = JSONObject(payload)
            validateReview(json, server, "device")
            require(json.getJSONObject("change").getString("kind") in listOf("alias", "remove"))
            review = Review(server, "device", payload)
        } catch (failure: Exception) { error = failure.message }
    }

    fun prepareDevice(id: String, change: JSONObject) = task {
        val server = selected ?: return@task
        val request = JSONObject().put("action", "prepare").put("id", id).put("change", change)
        val payload = withContext(Dispatchers.IO) { api.rpc(server, deviceOperation(request)) as String }
        reviewDevice(payload)
    }

    fun reviewEnrollment(payload: String) {
        try {
            val server = selected ?: return
            validateReview(JSONObject(payload), server, "enrollment")
            review = Review(server, "enrollment", payload)
        } catch (failure: Exception) {
            error = failure.message
        }
    }

    private fun validateReview(json: JSONObject, server: Server, kind: String) {
        require(
            json.getString("server_id") == enrollment?.getString("server_id") &&
                (kind == "device" || json.getInt("policy_version") == 1) &&
                json.getLong("expires_at") > System.currentTimeMillis() / 1000
        ) {
            "Review identity or expiry is invalid"
        }
        if (kind == "capability") require(json.getString("approver_id") == server.device)
    }

    fun decide(activity: MainActivity, approve: Boolean) {
        val current = review ?: return
        try {
            validateReview(JSONObject(current.payload), current.server, current.kind)
        } catch (failure: Exception) {
            error = failure.message
            return
        }
        val message =
            "xlatch.${current.kind}.decision.v1\n${if(approve) "approve" else "reject"}\n${current.payload}"
        review = null
        ApprovalKeys(current.server).sign(activity, message) { signature ->
            signature
                .onFailure { error = it.message }
                .onSuccess { signed ->
                    task {
                        val json = JSONObject(current.payload)
                        val request =
                            operation(
                                if (current.kind == "capability") "approval" else "enrollment",
                                "decide",
                            )
                        request
                            .getJSONObject("request")
                            .put(
                                "id",
                                json.getString(
                                    if (current.kind == "enrollment") "device_id" else "id"
                                ),
                            )
                            .put("approve", approve)
                            .put("signature", signed)
                        withContext(Dispatchers.IO) { api.rpc(current.server, if (current.kind == "device") deviceOperation(request.getJSONObject("request")) else request) }
                        refreshData(true)
                    }
                }
        }
    }

    fun bootstrap(activity: MainActivity, code: String) = task {
        val server = selected ?: error("Choose server")
        val tokenJson = JSONObject(code)
        val status =
            withContext(Dispatchers.IO) {
                api.rpc(server, operation("enrollment", "status")) as JSONObject
            }
        require(
            tokenJson.getString("purpose") == "xlatch.approval-bootstrap" &&
                tokenJson.getString("device_id") == server.device &&
                tokenJson.getString("server_id") == status.getString("server_id") &&
                tokenJson.getLong("expires_at") > System.currentTimeMillis() / 1000
        ) {
            "Bootstrap does not match this device or is expired"
        }
        val keys = ApprovalKeys(server)
        val public = withContext(Dispatchers.IO) { keys.publicKey() }
        val token = tokenJson.getString("token")
        keys.sign(
            activity,
            "xlatch.enrollment.enable.v1\n${status.getString("server_id")}\n${server.device}\n$token\n$public",
        ) { signed ->
            signed
                .onFailure { error = it.message }
                .onSuccess { signature ->
                    task {
                        val request = operation("enrollment", "enable")
                        request
                            .getJSONObject("request")
                            .put("token", token)
                            .put("public_key", public)
                            .put("signature", signature)
                        withContext(Dispatchers.IO) { api.rpc(server, request) }
                        refreshData(true)
                    }
                }
        }
    }

    override fun onCleared() {
        store.close()
    }
}
