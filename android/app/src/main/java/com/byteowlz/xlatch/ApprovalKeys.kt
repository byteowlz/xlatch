package com.byteowlz.xlatch

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/** Ordinary Ed25519 request keys never authorize protected changes. */
class ApprovalKeys(private val server: Server) {
    private val alias = "xlatch.approval.${server.id}"

    fun publicKey(): String {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (!store.containsAlias(alias)) {
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
                .apply {
                    initialize(
                        KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
                            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                            .setDigests(KeyProperties.DIGEST_SHA256)
                            .setUserAuthenticationRequired(true)
                            .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                            .setInvalidatedByBiometricEnrollment(true)
                            .build()
                    )
                }
                .generateKeyPair()
        }
        val private = store.getKey(alias, null) as java.security.PrivateKey
        val info =
            KeyFactory.getInstance(private.algorithm, "AndroidKeyStore")
                .getKeySpec(private, KeyInfo::class.java)
        @Suppress("DEPRECATION")
        require(info.isInsideSecureHardware) {
            "This device cannot provide the hardware-backed approval key required by xlatch."
        }
        return b64(rawPoint(store.getCertificate(alias).publicKey as ECPublicKey))
    }

    fun sign(activity: FragmentActivity, message: String, done: (kotlin.Result<String>) -> Unit) {
        try {
            publicKey()
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            val signature =
                Signature.getInstance("SHA256withECDSA").apply {
                    initSign(store.getKey(alias, null) as java.security.PrivateKey)
                }
            val prompt =
                BiometricPrompt(
                    activity,
                    ContextCompat.getMainExecutor(activity),
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationError(code: Int, text: CharSequence) {
                            done(kotlin.Result.failure(IllegalStateException(text.toString())))
                        }

                        override fun onAuthenticationSucceeded(
                            result: BiometricPrompt.AuthenticationResult
                        ) {
                            done(
                                runCatching {
                                    val signer =
                                        result.cryptoObject?.signature
                                            ?: error("Missing authenticated signing operation")
                                    signer.update(message.toByteArray(Charsets.UTF_8))
                                    b64(signer.sign())
                                }
                            )
                        }
                    },
                )
            prompt.authenticate(
                BiometricPrompt.PromptInfo.Builder()
                    .setTitle("Approve with xlatch")
                    .setSubtitle("Sign the exact request you reviewed")
                    .setAllowedAuthenticators(
                        androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
                    )
                    .setNegativeButtonText("Cancel")
                    .build(),
                BiometricPrompt.CryptoObject(signature),
            )
        } catch (error: Exception) {
            done(kotlin.Result.failure(error))
        }
    }
}
