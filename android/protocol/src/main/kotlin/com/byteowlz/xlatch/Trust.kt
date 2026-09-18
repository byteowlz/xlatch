package com.byteowlz.xlatch

import java.security.KeyStore
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.security.interfaces.ECPublicKey
import java.util.concurrent.TimeUnit
import javax.net.ssl.*
import okhttp3.OkHttpClient

class PinTrust(private val pin: String, private val keyPin: String?) : X509TrustManager {
    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
        throw CertificateException("Client certificates are not used")
    }

    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
        val leaf = chain.firstOrNull() ?: throw CertificateException("Missing server certificate")
        leaf.checkValidity()
        val matches =
            if (keyPin != null)
                sha256(
                    rawPoint(
                        leaf.publicKey as? ECPublicKey
                            ?: throw CertificateException("Expected P-256 server key")
                    )
                ) == keyPin
            else sha256(leaf.encoded) == pin
        if (!matches)
            throw CertificateException(
                "Server identity does not match the paired key. Scan a trusted new pairing code."
            )
        val anchors =
            KeyStore.getInstance(KeyStore.getDefaultType()).apply {
                load(null)
                setCertificateEntry("paired", leaf)
            }
        val factory =
            TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply {
                init(anchors)
            }
        factory.trustManagers
            .filterIsInstance<X509TrustManager>()
            .first()
            .checkServerTrusted(chain, authType)
    }
}

fun rawPoint(key: ECPublicKey): ByteArray {
    require(
        key.params.curve.field.fieldSize == 256 &&
            key.params.order.toString(16) ==
                "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551"
    ) {
        "Expected P-256 key"
    }
    fun coordinate(bytes: ByteArray) =
        ByteArray(32).also { destination ->
            require(bytes.size <= 33)
            val trimmed = if (bytes.size == 33) bytes.copyOfRange(1, 33) else bytes
            trimmed.copyInto(destination, 32 - trimmed.size)
        }
    return byteArrayOf(4) +
        coordinate(key.w.affineX.toByteArray()) +
        coordinate(key.w.affineY.toByteArray())
}

fun pinnedClient(pin: String, keyPin: String?): OkHttpClient {
    val manager = PinTrust(pin, keyPin)
    val tls = SSLContext.getInstance("TLS").apply { init(null, arrayOf(manager), null) }
    return OkHttpClient.Builder()
        .sslSocketFactory(tls.socketFactory, manager)
        .followRedirects(false)
        .followSslRedirects(false)
        .retryOnConnectionFailure(false)
        .connectTimeout(3, TimeUnit.SECONDS)
        .readTimeout(25, TimeUnit.SECONDS)
        .callTimeout(35, TimeUnit.SECONDS)
        .build()
}
