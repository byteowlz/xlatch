//! Native-independent fixtures over the same wire proof interface consumers use.
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use xlatch_wire_auth::{CROSSLATCH_V1, Enrollment, OQTO_PERSONAL_V1, SignedRequest};

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}
fn request() -> SignedRequest {
    SignedRequest {
        device_id: "device-a".into(),
        timestamp: 1000,
        nonce: "fixture_nonce_0001".into(),
        payload: "{ \"op\" : \"discover\" }".into(),
        signature: String::new(),
    }
}

#[test]
fn existing_crosslatch_wire_bytes_are_unchanged() -> Result<()> {
    let mut request = request();
    ensure!(
        request.signing_bytes()
            == b"xlatch.rpc.v1\ndevice-a\n1000\nfixture_nonce_0001\n{ \"op\" : \"discover\" }"
    );
    request.signature = STANDARD.encode(key().sign(&request.signing_bytes()).to_bytes());
    let public = STANDARD.encode(key().verifying_key().to_bytes());
    request.verify_for(CROSSLATCH_V1, &public, 1000)?;
    let roundtrip: SignedRequest = serde_json::from_str(&serde_json::to_string(&request)?)?;
    roundtrip.verify_for(CROSSLATCH_V1, &public, 1000)?;
    Ok(())
}

#[test]
fn proof_cannot_cross_application_domains_or_change_exact_payload() -> Result<()> {
    let public = STANDARD.encode(key().verifying_key().to_bytes());
    for domain in [CROSSLATCH_V1, OQTO_PERSONAL_V1] {
        let mut request = request();
        request.signature =
            STANDARD.encode(key().sign(&request.signing_bytes_for(domain)).to_bytes());
        request.verify_for(domain, &public, 1000)?;
        let other = if domain == CROSSLATCH_V1 {
            OQTO_PERSONAL_V1
        } else {
            CROSSLATCH_V1
        };
        ensure!(request.verify_for(other, &public, 1000).is_err());
        request.payload = "{\"op\":\"discover\"}".into();
        ensure!(request.verify_for(domain, &public, 1000).is_err());
    }
    Ok(())
}

#[test]
fn enrollment_is_possession_proof_in_one_domain_not_a_grant() -> Result<()> {
    let mut enrollment = Enrollment {
        token: "a".repeat(64),
        name: "Fixture phone".into(),
        public_key: STANDARD.encode(key().verifying_key().to_bytes()),
        signature: String::new(),
    };
    enrollment.signature = STANDARD.encode(
        key()
            .sign(&enrollment.signing_bytes_for(CROSSLATCH_V1))
            .to_bytes(),
    );
    enrollment.verify()?;
    ensure!(enrollment.verify_for(OQTO_PERSONAL_V1).is_err());
    enrollment.signature = STANDARD.encode(
        key()
            .sign(&enrollment.signing_bytes_for(OQTO_PERSONAL_V1))
            .to_bytes(),
    );
    enrollment.verify_for(OQTO_PERSONAL_V1)?;
    ensure!(enrollment.verify().is_err());
    enrollment.name = "Another phone".into();
    ensure!(enrollment.verify_for(OQTO_PERSONAL_V1).is_err());
    Ok(())
}

#[test]
fn malformed_bounds_key_clock_nonce_and_payload_are_denied() -> Result<()> {
    let public = STANDARD.encode(key().verifying_key().to_bytes());
    let mut signed = request();
    signed.signature = STANDARD.encode(key().sign(&signed.signing_bytes()).to_bytes());
    signed.verify_for(CROSSLATCH_V1, &public, 940)?;
    signed.verify_for(CROSSLATCH_V1, &public, 1060)?;
    for now in [939, 1061, i64::MIN, i64::MAX] {
        ensure!(signed.verify_for(CROSSLATCH_V1, &public, now).is_err());
    }
    for invalid in [
        "",
        "short",
        "contains spaces in nonce",
        "newline_in_nonce\n",
        &"a".repeat(121),
    ] {
        let mut bad = signed.clone();
        bad.nonce = invalid.into();
        ensure!(bad.verify_for(CROSSLATCH_V1, &public, 1000).is_err());
    }
    for invalid in ["", "not-base64", &"a".repeat(65)] {
        ensure!(signed.verify_for(CROSSLATCH_V1, invalid, 1000).is_err());
    }
    let mut bad = signed.clone();
    bad.signature = "a".repeat(129);
    ensure!(bad.verify_for(CROSSLATCH_V1, &public, 1000).is_err());
    let mut bad = signed.clone();
    bad.device_id = "device-a\nsecond".into();
    ensure!(bad.verify_for(CROSSLATCH_V1, &public, 1000).is_err());
    let mut bad = signed;
    bad.payload = "a".repeat(xlatch_wire_auth::MAX_PAYLOAD_BYTES + 1);
    ensure!(bad.verify_for(CROSSLATCH_V1, &public, 1000).is_err());
    Ok(())
}
