package com.byteowlz.xlatch

import java.security.interfaces.ECPublicKey
import javax.net.ssl.SSLException
import okhttp3.Request
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.Assert.*
import org.junit.Test

class TrustTest {
    private fun request(cert: HeldCertificate, pin: String, keyPin: String? = null): Int {
        MockWebServer().use { server ->
            server.useHttps(
                HandshakeCertificates.Builder().heldCertificate(cert).build().sslSocketFactory(),
                false,
            )
            server.enqueue(MockResponse().setBody("{}"))
            server.start()
            pinnedClient(pin, keyPin)
                .newCall(Request.Builder().url(server.url("/health")).build())
                .execute()
                .use {
                    return it.code
                }
        }
    }

    @Test
    fun acceptsPinnedCertificateAndRejectsWrongPin() {
        val cert =
            HeldCertificate.Builder()
                .commonName("localhost")
                .addSubjectAlternativeName("localhost")
                .ecdsa256()
                .build()
        assertEquals(200, request(cert, sha256(cert.certificate.encoded)))
        assertThrows(SSLException::class.java) { request(cert, "a".repeat(64)) }
    }

    @Test
    fun keyPinSurvivesRenewalButNeverFallsBackToLeafPin() {
        val first =
            HeldCertificate.Builder()
                .commonName("localhost")
                .addSubjectAlternativeName("localhost")
                .ecdsa256()
                .build()
        val renewed =
            HeldCertificate.Builder()
                .commonName("localhost")
                .addSubjectAlternativeName("localhost")
                .keyPair(first.keyPair)
                .serialNumber(2)
                .build()
        val key = sha256(rawPoint(first.keyPair.public as ECPublicKey))
        assertEquals(200, request(renewed, sha256(first.certificate.encoded), key))
        assertThrows(SSLException::class.java) {
            request(renewed, sha256(renewed.certificate.encoded), "b".repeat(64))
        }
    }

    @Test
    fun pinnedCertificateStillRequiresMatchingHostname() {
        val cert =
            HeldCertificate.Builder()
                .commonName("wrong.example")
                .addSubjectAlternativeName("wrong.example")
                .ecdsa256()
                .build()
        assertThrows(SSLException::class.java) { request(cert, sha256(cert.certificate.encoded)) }
    }
}
