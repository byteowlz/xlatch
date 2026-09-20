package com.byteowlz.xlatch

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import org.json.JSONObject

/** Copy transient content grants now, before the share activity loses access. */
fun readShare(context: Context, intent: Intent): List<JSONObject> {
    if (intent.action !in listOf(Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE))
        return emptyList()
    Store(context).use { it.pruneFiles() }
    @Suppress("DEPRECATION")
    val uris =
        if (intent.action == Intent.ACTION_SEND_MULTIPLE)
            intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)?.toList() ?: emptyList()
        else listOfNotNull(intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
    require(uris.size <= 8) { "Share up to eight files at a time." }
    if (uris.isNotEmpty())
        return uris.map { uri ->
            require(uri.scheme == "content") {
                "Only Android content-provider files are supported."
            }
            val mime =
                context.contentResolver.getType(uri) ?: intent.type ?: "application/octet-stream"
            val name =
                context.contentResolver
                    .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                    ?.use { if (it.moveToFirst()) it.getString(0) else null } ?: "shared-file"
            val staged = SharedFiles.file(context, java.util.UUID.randomUUID().toString())
            try {
                context.contentResolver.openInputStream(uri)?.use { source ->
                    staged.outputStream().use { output -> source.copyTo(output, 1024 * 1024) }
                } ?: error("Could not read the shared file")
                val file = JSONObject().put("name", name).put("mime_type", mime)
                val input = JSONObject().put("mime_type", mime).put("file", file)
                if (staged.length() <= MAX_FILE) {
                    file.put("data_base64", b64(staged.readBytes()))
                    check(staged.delete()) { "Could not clean up staged content" }
                } else {
                    file.put("size", staged.length())
                    input.put("_local_file", staged.name)
                }
                input
            } catch (error: Exception) {
                staged.delete()
                throw error
            }
        }
    val text =
        intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
            ?: error("The sharing app did not provide readable text or a file.")
    require(text.toByteArray().size <= 100000) { "Shared text is too large." }
    return listOf(
        JSONObject()
            .put("text", text)
            .put(
                "mime_type",
                if (text.trim().matches(Regex("https?://\\S+"))) "text/uri-list" else "text/plain",
            )
    )
}
