package com.byteowlz.xlatch

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONArray
import org.json.JSONObject

class Vault {
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("xlatch.storage", null) as? SecretKey)?.let {
            return it
        }
        synchronized(Vault::class.java) {
            (store.getKey("xlatch.storage", null) as? SecretKey)?.let {
                return it
            }
            return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
                .apply {
                    init(
                        KeyGenParameterSpec.Builder(
                                "xlatch.storage",
                                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                            )
                            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                            .setUnlockedDeviceRequired(true)
                            .build()
                    )
                }
                .generateKey()
        }
    }

    fun seal(value: String): ByteArray {
        val cipher =
            Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
        return cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
    }

    fun open(bytes: ByteArray): String {
        require(bytes.size >= 28)
        val cipher =
            Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
            }
        return cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8)
    }
}

class Store(context: Context) : SQLiteOpenHelper(context.applicationContext, "xlatch.db", null, 1) {
    private val vault = Vault()
    private val filesContext = context.applicationContext

    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE records(kind TEXT NOT NULL,id TEXT NOT NULL,value BLOB NOT NULL,PRIMARY KEY(kind,id))"
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        error("Unsupported database upgrade")
    }

    fun put(kind: String, id: String, value: String) {
        writableDatabase.insertWithOnConflict(
            "records",
            null,
            ContentValues().apply {
                put("kind", kind)
                put("id", id)
                put("value", vault.seal(value))
            },
            SQLiteDatabase.CONFLICT_REPLACE,
        )
    }

    fun get(kind: String, id: String): String? =
        readableDatabase
            .query(
                "records",
                arrayOf("value"),
                "kind=? AND id=?",
                arrayOf(kind, id),
                null,
                null,
                null,
            )
            .use { if (it.moveToFirst()) vault.open(it.getBlob(0)) else null }

    fun all(kind: String): List<Pair<String, String>> =
        readableDatabase
            .query(
                "records",
                arrayOf("id", "value"),
                "kind=?",
                arrayOf(kind),
                null,
                null,
                "rowid DESC",
            )
            .use { cursor ->
                buildList {
                    while (cursor.moveToNext()) add(
                        cursor.getString(0) to vault.open(cursor.getBlob(1))
                    )
                }
            }

    fun remove(kind: String, id: String) {
        val saved = if (kind == "outbox") get(kind, id)?.let(::JSONObject)?.optJSONObject("input") else null
        writableDatabase.delete("records", "kind=? AND id=?", arrayOf(kind, id))
        saved?.optString("_local_file")?.takeIf { it.isNotEmpty() }?.let { SharedFiles.file(filesContext, it).delete() }
    }

    fun pruneFiles() {
        val records = all("outbox").map { it.first to JSONObject(it.second) }
        val retained = mutableSetOf<String>()
        val cutoff = System.currentTimeMillis() - java.util.concurrent.TimeUnit.DAYS.toMillis(7)
        for ((id, item) in records) {
            val name = item.optJSONObject("input")?.optString("_local_file").orEmpty()
            if (name.isEmpty()) continue
            if (item.getLong("created") < cutoff) {
                item.remove("input")
                item.put("status", "paused").put("error", "Share expired after seven days; content removed")
                put("outbox", id, item.toString())
                SharedFiles.file(filesContext, name).delete()
            } else retained.add(name)
        }
        val orphanCutoff = System.currentTimeMillis() - java.util.concurrent.TimeUnit.DAYS.toMillis(1)
        java.io.File(filesContext.noBackupFilesDir, "shared-files").listFiles()?.forEach { file ->
            if (file.name !in retained && file.lastModified() < orphanCutoff) file.delete()
        }
    }

    fun servers() = all("server").map { Server.parse(JSONObject(it.second)) }

    fun server(id: String) =
        get("server", id)?.let { Server.parse(JSONObject(it)) }
            ?: error("Server is no longer paired")

    fun seed(id: String): ByteArray =
        Base64.getDecoder().decode(get("key", id) ?: error("Device key is unavailable; pair again"))

    fun saveServer(server: Server, seed: ByteArray) {
        writableDatabase.beginTransaction()
        try {
            put("server", server.id, server.json().toString())
            put("key", server.id, b64(seed))
            writableDatabase.setTransactionSuccessful()
        } finally {
            writableDatabase.endTransaction()
        }
    }

    fun updateServer(server: Server) {
        put("server", server.id, server.json().toString())
    }

    fun catalog(server: Server) = JSONArray(get("catalog", server.id) ?: "[]").objects()

    fun enabled(server: Server, action: JSONObject) =
        get("disabled", server.id + ":" + action.getJSONObject("manifest").getString("id")) !=
            action.getString("revision")

    fun enable(server: Server, action: JSONObject, on: Boolean) {
        val id = server.id + ":" + action.getJSONObject("manifest").getString("id")
        if (on) remove("disabled", id) else put("disabled", id, action.getString("revision"))
    }

    fun queue(server: Server, action: JSONObject, input: JSONObject): String {
        require(enabled(server, action)) { "This action is disabled" }
        val id = java.util.UUID.randomUUID().toString()
        val item =
            JSONObject()
                .put("id", id)
                .put("server", server.id)
                .put("device", server.device)
                .put("pin", server.pin)
                .put("capability", action.getJSONObject("manifest").getString("id"))
                .put("revision", action.getString("revision"))
                .put("input", JSONObject(input.toString()))
                .put("status", "queued")
                .put("created", System.currentTimeMillis())
        writableDatabase.beginTransaction()
        try {
            val existing = all("outbox")
            require(
                existing.size < 50 &&
                    existing.sumOf { it.second.toByteArray().size.toLong() } +
                        item.toString().toByteArray().size <= 64L * 1024 * 1024
            ) {
                "Outbox is full. Remove delivered items first."
            }
            input.optString("_local_file").takeIf { it.isNotEmpty() }?.let { name ->
                SharedFiles.file(filesContext, name).copyTo(SharedFiles.file(filesContext, id))
                item.getJSONObject("input").put("_local_file", id)
            }
            put("outbox", id, item.toString())
            writableDatabase.setTransactionSuccessful()
        } finally {
            writableDatabase.endTransaction()
        }
        return id
    }
}
