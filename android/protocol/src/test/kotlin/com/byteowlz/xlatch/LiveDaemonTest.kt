package com.byteowlz.xlatch

import java.io.File
import java.util.UUID
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Opt-in real Rust daemon check; ticket is a short-lived file outside the repo. */
class LiveDaemonTest {
    @Test
    fun pairsDiscoversInvokesAndRetrievesUsingSignedEnvelope() {
        val path = System.getenv("XLATCH_TEST_TICKET")
        assumeTrue("No isolated daemon ticket supplied", path != null)
        val ticket = Ticket.parse(File(path!!).readText())
        val client = pinnedClient(ticket.pin, null)
        fun post(path: String, body: JSONObject): String =
            client
                .newCall(
                    Request.Builder()
                        .url(ticket.urls.first() + path)
                        .post(body.toString().toRequestBody("application/json".toMediaType()))
                        .build()
                )
                .execute()
                .use { response ->
                    check(response.isSuccessful) { "HTTP ${response.code}" }
                    response.body!!.string()
                }
        val seed = newSeed()
        val public = publicKey(seed)
        val name = "Android JVM conformance"
        val pair =
            JSONObject(
                post(
                    "/v1/pair",
                    JSONObject()
                        .put("token", ticket.token)
                        .put("name", name)
                        .put("public_key", public)
                        .put(
                            "signature",
                            sign(
                                seed,
                                "xlatch.pair.v1\n${ticket.token}\n$public\n$name".toByteArray(),
                            ),
                        ),
                )
            )
        fun rpc(payload: JSONObject): String {
            val text = payload.toString()
            val nonce = UUID.randomUUID().toString()
            val time = System.currentTimeMillis() / 1000
            val device = pair.getString("id")
            return post(
                "/v1/rpc",
                JSONObject()
                    .put("device_id", device)
                    .put("timestamp", time)
                    .put("nonce", nonce)
                    .put("payload", text)
                    .put("signature", sign(seed, rpcBytes(device, time, nonce, text))),
            )
        }
        val action =
            JSONArray(rpc(JSONObject().put("op", "discover"))).objects().first {
                it.getJSONObject("manifest").getString("id") == "echo"
            }
        val payload =
            JSONObject()
                .put("op", "invoke")
                .put("capability_id", "echo")
                .put("revision", action.getString("revision"))
                .put(
                    "input",
                    JSONObject().put("text", "Android → xlatch").put("mime_type", "text/plain"),
                )
                .put("idempotency_key", UUID.randomUUID().toString())
        val first = JSONObject(rpc(payload))
        val repeated = JSONObject(rpc(payload))
        assertEquals(first.getString("id"), repeated.getString("id"))
        var job = first
        repeat(30) {
            if (job.getString("status") in listOf("queued", "running")) {
                Thread.sleep(100)
                job =
                    JSONObject(rpc(JSONObject().put("op", "job").put("id", first.getString("id"))))
            }
        }
        assertEquals("succeeded", job.getString("status"))
        assertEquals("Android → xlatch", job.getJSONObject("result").getString("text"))
        seed.fill(0)
    }
}
