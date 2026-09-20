package com.byteowlz.xlatch

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.work.*
import java.io.IOException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.withContext
import org.json.JSONObject

object Uploads {
    val progress = MutableStateFlow<Map<String, Float>>(emptyMap())
}

fun scheduleDelivery(context: Context) {
    val request =
        OneTimeWorkRequestBuilder<DeliveryWorker>()
            .setConstraints(
                Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()
            )
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 15, TimeUnit.SECONDS)
            .build()
    WorkManager.getInstance(context)
        .enqueueUniqueWork("xlatch-delivery", ExistingWorkPolicy.APPEND_OR_REPLACE, request)
}

class DeliveryWorker(context: Context, parameters: WorkerParameters) :
    CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result =
        withContext(Dispatchers.IO) {
            val store = Store(applicationContext)
            try {
                store.pruneFiles()
                var retry = false
                for ((id, body) in store.all("outbox")) {
                    val item = JSONObject(body)
                    if (item.getString("status") != "queued") continue
                    try {
                        require(
                            System.currentTimeMillis() - item.getLong("created") <=
                                TimeUnit.DAYS.toMillis(7)
                        ) {
                            "Share expired after seven days"
                        }
                        val server = store.server(item.getString("server"))
                        require(
                            server.device == item.getString("device") &&
                                server.pin == item.getString("pin")
                        ) {
                            "Pairing changed; review this share"
                        }
                        val api = Api(store)
                        val action =
                            api.discover(server).objects().find {
                                it.getJSONObject("manifest").getString("id") ==
                                    item.getString("capability") &&
                                    it.getString("revision") == item.getString("revision")
                            } ?: error("Action changed or its grant was removed")
                        require(store.enabled(server, action)) { "Action disabled on this phone" }
                        if (
                            JSONObject(store.get("outbox", id) ?: continue).getString("status") !=
                                "queued"
                        )
                            continue
                        val input = uploadFile(applicationContext, store, api, server, item, action)
                        val job =
                            api.rpc(
                                server,
                                JSONObject()
                                    .put("op", "invoke")
                                    .put("capability_id", item.getString("capability"))
                                    .put("revision", item.getString("revision"))
                                    .put("input", input)
                                    .put("idempotency_key", id),
                            ) { fraction ->
                                if (!item.getJSONObject("input").has("_local_file")) Uploads.progress.value = Uploads.progress.value + (id to fraction)
                            } as JSONObject
                        val localFile = item.getJSONObject("input").optString("_local_file")
                        item.put("status", "sent").put("job", job.getString("id")).remove("input")
                        store.put("outbox", id, item.toString())
                        if (localFile.isNotEmpty()) SharedFiles.file(applicationContext, localFile).delete()
                        val poll =
                            OneTimeWorkRequestBuilder<JobWorker>()
                                .setInputData(
                                    workDataOf("server" to server.id, "job" to job.getString("id"))
                                )
                                .setConstraints(
                                    Constraints.Builder()
                                        .setRequiredNetworkType(NetworkType.CONNECTED)
                                        .build()
                                )
                                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                                .build()
                        WorkManager.getInstance(applicationContext)
                            .enqueueUniqueWork(
                                "result-${server.id}-${job.getString("id")}",
                                ExistingWorkPolicy.KEEP,
                                poll,
                            )
                    } catch (_: java.security.GeneralSecurityException) {
                        retry = true
                    } catch (error: Exception) {
                        val permanent =
                            error is SSLException ||
                                (error is ApiFailure &&
                                    error.status in 400..499 &&
                                    error.status != 429) ||
                                error !is IOException
                        if (permanent) item.put("status", "paused") else retry = true
                        item.put("error", error.message ?: "Delivery failed")
                        val latest = store.get("outbox", id)?.let(::JSONObject)
                        if (latest?.optString("status") == "queued")
                            store.put("outbox", id, item.toString())
                    } finally {
                        Uploads.progress.value = Uploads.progress.value - id
                    }
                }
                if (retry) Result.retry() else Result.success()
            } catch (_: java.security.GeneralSecurityException) {
                Result.retry()
            } finally {
                store.close()
            }
        }
}

class JobWorker(context: Context, parameters: WorkerParameters) :
    CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result =
        withContext(Dispatchers.IO) {
            val store = Store(applicationContext)
            try {
                val server =
                    store.server(
                        inputData.getString("server") ?: return@withContext Result.failure()
                    )
                val id = inputData.getString("job") ?: return@withContext Result.failure()
                val job =
                    Api(store).rpc(server, JSONObject().put("op", "job").put("id", id))
                        as JSONObject
                if (job.getString("status") in listOf("queued", "running"))
                    return@withContext Result.retry()
                store.put("result", server.id + ":" + id, job.toString())
                val manager = applicationContext.getSystemService(NotificationManager::class.java)
                manager.createNotificationChannel(
                    NotificationChannel(
                        "jobs",
                        "Job results",
                        NotificationManager.IMPORTANCE_DEFAULT,
                    )
                )
                if (
                    android.os.Build.VERSION.SDK_INT < 33 ||
                        ContextCompat.checkSelfPermission(
                            applicationContext,
                            Manifest.permission.POST_NOTIFICATIONS,
                        ) == PackageManager.PERMISSION_GRANTED
                ) {
                    val launch =
                        android.app.PendingIntent.getActivity(
                            applicationContext,
                            0,
                            android.content.Intent(applicationContext, MainActivity::class.java),
                            android.app.PendingIntent.FLAG_IMMUTABLE or
                                android.app.PendingIntent.FLAG_UPDATE_CURRENT,
                        )
                    manager.notify(
                        id.hashCode(),
                        NotificationCompat.Builder(applicationContext, "jobs")
                            .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
                            .setContentTitle("xlatch job ${job.getString("status")}")
                            .setContentText("Open xlatch to view the result.")
                            .setContentIntent(launch)
                            .setAutoCancel(true)
                            .build(),
                    )
                }
                Result.success()
            } catch (error: SSLException) {
                Result.failure()
            } catch (error: ApiFailure) {
                if (error.status in 400..499 && error.status != 429) Result.failure()
                else Result.retry()
            } catch (error: IOException) {
                Result.retry()
            } catch (_: java.security.GeneralSecurityException) {
                Result.retry()
            } catch (error: Exception) {
                Result.failure()
            } finally {
                store.close()
            }
        }
}
