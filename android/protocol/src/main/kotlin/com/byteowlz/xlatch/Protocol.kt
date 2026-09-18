package com.byteowlz.xlatch

import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import okhttp3.HttpUrl.Companion.toHttpUrl
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import org.json.JSONArray
import org.json.JSONObject

const val MAX_FILE = 4 * 1024 * 1024
const val MAX_RESPONSE = 8 * 1024 * 1024

fun b64(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)

fun sha256(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }

fun JSONArray.strings(): List<String> = (0 until length()).map { getString(it) }

fun origin(value: String): String {
    val url = value.toHttpUrl()
    require(
        url.isHttps &&
            url.username.isEmpty() &&
            url.password.isEmpty() &&
            url.encodedPath == "/" &&
            url.query == null &&
            url.fragment == null
    ) {
        "Expected an HTTPS server origin without credentials, path or query."
    }
    return url.toString().trimEnd('/')
}

fun rpcBytes(device: String, time: Long, nonce: String, payload: String) =
    "xlatch.rpc.v1\n$device\n$time\n$nonce\n$payload".toByteArray(Charsets.UTF_8)

fun sign(seed: ByteArray, bytes: ByteArray): String {
    val signer = Ed25519Signer()
    signer.init(true, Ed25519PrivateKeyParameters(seed, 0))
    signer.update(bytes, 0, bytes.size)
    return b64(signer.generateSignature())
}

fun newSeed(): ByteArray = ByteArray(32).also { SecureRandom().nextBytes(it) }

fun publicKey(seed: ByteArray): String =
    b64(Ed25519PrivateKeyParameters(seed, 0).generatePublicKey().encoded)

data class Ticket(val urls: List<String>, val pin: String, val token: String) {
    companion object {
        fun parse(text: String, now: Long = System.currentTimeMillis() / 1000): Ticket {
            require(text.length <= 16384) { "Pairing code is too large." }
            val json = JSONObject(text)
            require(json.getInt("version") == 1 && json.getLong("expires_at") >= now) {
                "Pairing code is invalid or expired."
            }
            val urls =
                listOf(json.getString("url")) +
                    (json.optJSONArray("urls")?.strings() ?: emptyList())
            require(urls.size <= 9) { "Too many server addresses." }
            val pin = json.getString("pin").lowercase()
            val token = json.getString("token")
            require(pin.matches(Regex("[0-9a-f]{64}")) && token.length == 64) {
                "Invalid pairing identity."
            }
            return Ticket(urls.map(::origin).distinct(), pin, token)
        }
    }
}

data class Server(
    val id: String,
    val name: String,
    val device: String,
    val pin: String,
    val keyPin: String?,
    val urls: List<String>,
) {
    fun json() =
        JSONObject()
            .put("id", id)
            .put("name", name)
            .put("device", device)
            .put("pin", pin)
            .put("key_pin", keyPin)
            .put("urls", JSONArray(urls))

    companion object {
        fun parse(json: JSONObject) =
            Server(
                json.getString("id"),
                json.getString("name"),
                json.getString("device"),
                json.getString("pin"),
                json.optString("key_pin").takeIf { it.isNotEmpty() },
                json.getJSONArray("urls").strings(),
            )
    }
}

fun accepts(action: JSONObject, mime: String): Boolean =
    action.getJSONObject("manifest").getJSONArray("accepts").strings().any {
        it == mime ||
            it == "*/*" ||
            (it.endsWith("/*") && mime.startsWith(it.substringBefore('/') + "/"))
    }
