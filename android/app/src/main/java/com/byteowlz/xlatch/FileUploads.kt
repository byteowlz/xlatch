package com.byteowlz.xlatch

import android.content.Context
import java.io.File
import java.io.RandomAccessFile
import java.util.UUID
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import org.json.JSONObject

/** UUID-only private paths; never accept a host filesystem path from shared content. */
object SharedFiles {
    fun file(context: Context, id: String): File {
        require(UUID.fromString(id).toString().equals(id, ignoreCase = true)) { "Invalid saved file identity" }
        val directory = File(context.noBackupFilesDir, "shared-files")
        check(directory.isDirectory || directory.mkdirs()) { "Shared file storage is unavailable" }
        return File(directory, id)
    }
}

/** Resume immutable, authenticated chunks; the invocation contains only a file reference. */
suspend fun uploadFile(context: Context, store: Store, api: Api, server: Server, item: JSONObject, action: JSONObject): JSONObject {
    val input = JSONObject(item.getJSONObject("input").toString())
    val name = input.optString("_local_file")
    if (name.isEmpty()) return input
    val manifest = action.getJSONObject("manifest")
    require(manifest.optString("file_input") == "path" || manifest.getJSONObject("execution").getString("kind") == "save_file") {
        "This action needs a file-path adapter for large files. Your file remains in Outbox."
    }
    val id = item.getString("id")
    fun checkQueued() {
        check(store.get("outbox", id)?.let { JSONObject(it).optString("status") } == "queued") { "Delivery was cancelled" }
    }
    val metadata = input.getJSONObject("file")
    RandomAccessFile(SharedFiles.file(context, name), "r").use { file ->
        val size = file.length()
        require(size == metadata.getLong("size")) { "Saved file changed" }
        fun request(body: JSONObject) = api.rpc(server, JSONObject().put("op", "upload").put("request", body)) as JSONObject
        var state = request(JSONObject().put("action", "begin").put("id", id).put("name", metadata.getString("name")).put("mime_type", metadata.getString("mime_type")).put("size", size))
        val chunkSize = state.getInt("chunk_bytes")
        require(chunkSize in 1..1024 * 1024 && state.getLong("offset") in 0..size) { "Invalid upload response" }
        while (state.getLong("offset") < size) {
            currentCoroutineContext().ensureActive()
            checkQueued()
            val offset = state.getLong("offset")
            file.seek(offset)
            val bytes = ByteArray(minOf(chunkSize.toLong(), size - offset).toInt())
            file.readFully(bytes)
            val next = request(JSONObject().put("action", "chunk").put("id", id).put("offset", offset).put("data_base64", b64(bytes)))
            require(next.getLong("offset") == offset + bytes.size && next.getInt("chunk_bytes") == chunkSize) { "Upload offset changed" }
            state = next
            Uploads.progress.value = Uploads.progress.value + (id to (state.getLong("offset").toDouble() / size).toFloat())
        }
        require(state.getBoolean("complete")) { "Upload incomplete" }
        currentCoroutineContext().ensureActive()
        checkQueued()
    }
    input.remove("_local_file")
    metadata.put("artifact_id", id)
    return input
}
