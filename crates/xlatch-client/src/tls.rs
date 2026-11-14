//! Pinned self-signed TLS, without disabling hostname, validity or handshake checks.
use anyhow::{Result, ensure};
use rustls::{
    DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest as _, Sha256};
use std::{sync::Arc, time::Duration};
use x509_parser::prelude::FromDer as _;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug)]
struct Pin {
    certificate: String,
    key: Option<String>,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let valid = self.key.as_ref().map_or_else(
            || digest(end_entity.as_ref()) == self.certificate,
            |pin| {
                x509_parser::certificate::X509Certificate::from_der(end_entity.as_ref()).is_ok_and(
                    |(_, cert)| digest(cert.public_key().subject_public_key.data.as_ref()) == *pin,
                )
            },
        );
        if !valid {
            return Err(rustls::Error::General("xlatch server pin mismatch".into()));
        }
        // Only the out-of-band pinned leaf (or remembered key) may be an anchor.
        let mut roots = RootCertStore::empty();
        roots.add(end_entity.clone())?;
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|error| rustls::Error::General(error.to_string()))?;
        verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn http(pin: &str, key_pin: Option<String>, timeout: u64) -> Result<reqwest::Client> {
    validate_pin(pin)?;
    if let Some(key) = &key_pin {
        validate_pin(key)?;
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(Pin {
        certificate: pin.into(),
        key: key_pin,
    }))
    .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(timeout))
        .build()?)
}

pub fn validate_pin(pin: &str) -> Result<()> {
    ensure!(
        pin.len() == 64
            && pin
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "expected lowercase SHA-256 pin"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_certificate_and_conflicting_key_pin_fail_closed() -> Result<()> {
        let key = rcgen::KeyPair::generate()?;
        let mut parameters = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?;
        parameters.not_before = rcgen::date_time_ymd(2000, 1, 1);
        parameters.not_after = rcgen::date_time_ymd(2001, 1, 1);
        let certificate = parameters.self_signed(&key)?;
        let name = ServerName::try_from("127.0.0.1")?;
        let expired = Pin {
            certificate: digest(certificate.der()),
            key: None,
        };
        ensure!(
            expired
                .verify_server_cert(certificate.der(), &[], &name, &[], UnixTime::now())
                .is_err(),
            "expired certificate trusted"
        );
        let certificate =
            rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?.self_signed(&key)?;
        let conflicting = Pin {
            certificate: digest(certificate.der()),
            key: Some("0".repeat(64)),
        };
        ensure!(
            conflicting
                .verify_server_cert(certificate.der(), &[], &name, &[], UnixTime::now())
                .is_err(),
            "matching certificate overrode remembered key"
        );
        Ok(())
    }
}
