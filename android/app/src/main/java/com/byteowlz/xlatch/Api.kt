package com.byteowlz.xlatch

import java.io.IOException
import java.util.UUID
import javax.net.ssl.*
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okio.BufferedSink
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

class ApiFailure(val status: Int, message: String) : IOException(message)

class Api(private val store: Store) {
    data class Route(val url: String, val keyPin: String?, val urls: List<String>)

    private fun request(
        client: OkHttpClient,
        url: String,
        body: JSONObject? = null,
        progress: (Float) -> Unit = {},
    ): Any {
        val builder = Request.Builder().url(url)
        if (body != null) {
            val bytes = body.toString().toByteArray(Charsets.UTF_8)
            require(bytes.size <= MAX_RESPONSE) { "Request exceeds 8 MiB" }
            builder.post(
                object : RequestBody() {
                    override fun contentType() = "application/json".toMediaType()

                    override fun contentLength() = bytes.size.toLong()

                    override fun writeTo(sink: BufferedSink) {
                        var offset = 0
                        while (offset < bytes.size) {
                            val count = minOf(32768, bytes.size - offset)
                            sink.write(bytes, offset, count)
                            offset += count
                            progress(offset.toFloat() / bytes.size)
                        }
                    }
                }
            )
        }
        client.newCall(builder.build()).execute().use { response ->
            val source = response.body?.source() ?: throw IOException("Empty server response")
            val buffer = okio.Buffer()
            while (buffer.size <= MAX_RESPONSE) {
                if (source.read(buffer, minOf(32768L, MAX_RESPONSE + 1L - buffer.size)) == -1L)
                    break
            }
            require(buffer.size <= MAX_RESPONSE) { "Server response exceeds 8 MiB" }
            val value = JSONTokener(buffer.readUtf8()).nextValue()
            if (!response.isSuccessful)
                throw ApiFailure(
                    response.code,
                    (value as? JSONObject)?.optString("error")
                        ?: "Server rejected request (${response.code})",
                )
            return value
        }
    }

    fun route(server: Server): Route {
        val client = pinnedClient(server.pin, server.keyPin)
        var last: IOException? = null
        for (address in server.urls.distinct().take(9)) {
            try {
                val url = origin(address)
                val health = request(client, "$url/health") as JSONObject
                require(health.getString("name") == "xlatch" && health.getInt("version") == 1) {
                    "Not an xlatch server"
                }
                val key = health.optString("key_pin").takeIf { it.isNotEmpty() }
                require(key == null || key.matches(Regex("[0-9a-f]{64}"))) { "Invalid TLS key pin" }
                require(server.keyPin == null || key == server.keyPin) { "Server key changed" }
                val announced = health.optJSONArray("urls")?.strings() ?: emptyList()
                require(announced.size <= 9)
                val urls = (listOf(url) + announced.map(::origin) + server.urls).distinct().take(9)
                return Route(url, server.keyPin ?: key, urls)
            } catch (error: SSLException) {
                throw error
            } catch (error: IOException) {
                last = error
            }
        }
        throw last ?: IOException("No reachable server address")
    }

    fun pair(text: String, name: String): Server {
        require(name.isNotBlank() && name.length <= 80)
        val ticket = Ticket.parse(text)
        val id = UUID.randomUUID().toString()
        val seed = newSeed()
        try {
            val temporary =
                Server(id, ticket.urls.first().toHttpUrl().host, "", ticket.pin, null, ticket.urls)
            val route = route(temporary)
            val public = publicKey(seed)
            val bytes =
                "xlatch.pair.v1\n${ticket.token}\n$public\n$name".toByteArray(Charsets.UTF_8)
            val response =
                request(
                    pinnedClient(ticket.pin, route.keyPin),
                    "${route.url}/v1/pair",
                    JSONObject()
                        .put("token", ticket.token)
                        .put("name", name)
                        .put("public_key", public)
                        .put("signature", sign(seed, bytes)),
                )
                    as JSONObject
            val server =
                temporary.copy(
                    device = response.getString("id"),
                    keyPin = route.keyPin,
                    urls = route.urls,
                )
            store.saveServer(server, seed)
            return server
        } finally {
            seed.fill(0)
        }
    }

    fun rpc(server: Server, payload: JSONObject, progress: (Float) -> Unit = {}): Any {
        val route = route(server)
        store.updateServer(server.copy(keyPin = route.keyPin, urls = route.urls))
        val text = payload.toString()
        val nonce = UUID.randomUUID().toString()
        val time = System.currentTimeMillis() / 1000
        val seed = store.seed(server.id)
        val signature =
            try {
                sign(seed, rpcBytes(server.device, time, nonce, text))
            } finally {
                seed.fill(0)
            }
        return request(
            pinnedClient(server.pin, route.keyPin),
            "${route.url}/v1/rpc",
            JSONObject()
                .put("device_id", server.device)
                .put("timestamp", time)
                .put("nonce", nonce)
                .put("payload", text)
                .put("signature", signature),
            progress,
        )
    }

    fun discover(server: Server): JSONArray =
        (rpc(server, JSONObject().put("op", "discover")) as JSONArray).also {
            store.put("catalog", server.id, it.toString())
        }
}
