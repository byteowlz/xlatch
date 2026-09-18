package com.byteowlz.xlatch

import java.util.Base64
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ProtocolTest {
    private fun hex(s: String) = s.chunked(2).map { it.toInt(16).toByte() }.toByteArray()

    @Test
    fun rfc8032Ed25519Vector() {
        val seed = hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
        assertArrayEquals(
            hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"),
            Base64.getDecoder().decode(publicKey(seed)),
        )
        assertArrayEquals(
            hex(
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
            ),
            Base64.getDecoder().decode(sign(seed, byteArrayOf())),
        )
    }

    @Test
    fun envelopePreservesExactPayload() {
        val payload = "{ \"text\":\"hello\\n世界\",\"op\":\"invoke\" }"
        assertEquals(
            "xlatch.rpc.v1\ndevice\n123\nnonce\n$payload",
            rpcBytes("device", 123, "nonce", payload).toString(Charsets.UTF_8),
        )
    }

    @Test
    fun rejectsUnsafeOrigins() {
        for (url in
            listOf(
                "http://localhost",
                "https://user:secret@localhost",
                "https://localhost/path",
                "https://localhost/?token=abc",
                "https://localhost/#fragment",
            )) assertThrows(IllegalArgumentException::class.java) { origin(url) }
        assertEquals("https://100.64.0.12:7898", origin("https://100.64.0.12:7898/"))
    }

    @Test
    fun pairingExpiryAndPinAreChecked() {
        val ticket =
            JSONObject()
                .put("version", 1)
                .put("url", "https://localhost")
                .put("token", "t".repeat(64))
                .put("pin", "a".repeat(64))
                .put("expires_at", 100)
        assertEquals("a".repeat(64), Ticket.parse(ticket.toString(), 99).pin)
        assertThrows(IllegalArgumentException::class.java) { Ticket.parse(ticket.toString(), 101) }
        ticket.put("pin", "bad")
        assertThrows(IllegalArgumentException::class.java) { Ticket.parse(ticket.toString(), 99) }
    }
}
