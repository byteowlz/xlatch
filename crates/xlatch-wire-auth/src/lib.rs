//! Store-independent enrollment and exact-payload signature proofs.
//!
//! A successful proof is NOT authorization: the embedding store must atomically
//! consume enrollment tickets, persist replay nonces, enforce current grants,
//! check revocation and bind resource ownership. No service, database, scheduler
//! or private-key storage is provided here.

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Maximum signed payload size; HTTP/frame adapters must also bound input.
pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Compiled signing context, selected by the server rather than a wire field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SigningDomain {
    enrollment: &'static str,
    request: &'static str,
}

/// Existing `CrossLatch` v1 domain; its signing bytes remain unchanged.
pub const CROSSLATCH_V1: SigningDomain = SigningDomain {
    enrollment: "xlatch.pair.v1",
    request: "xlatch.rpc.v1",
};

/// Oqto's direct personal-host context, distinct from `CrossLatch` capabilities.
pub const OQTO_PERSONAL_V1: SigningDomain = SigningDomain {
    enrollment: "oqto.personal.pair.v1",
    request: "oqto.personal.rpc.v1",
};

/// Device proof of possession; a ticket store must separately consume `token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    /// Single-use enrollment secret received through the out-of-band QR.
    pub token: String,
    /// User-facing label, never an authority or identity credential.
    pub name: String,
    /// Base64 raw 32-byte Ed25519 public key.
    pub public_key: String,
    /// Base64 signature over the domain's exact enrollment signing bytes.
    pub signature: String,
}

impl Enrollment {
    /// Exact UTF-8 enrollment bytes; do not sign reconstructed JSON.
    #[must_use]
    pub fn signing_bytes_for(&self, domain: SigningDomain) -> Vec<u8> {
        format!(
            "{}\n{}\n{}\n{}",
            domain.enrollment, self.token, self.public_key, self.name
        )
        .into_bytes()
    }

    /// Verify an existing `CrossLatch` v1 enrollment proof.
    ///
    /// # Errors
    /// Rejects invalid labels, secret lengths, keys and signatures. Ticket
    /// ownership, expiry, consumption and grants remain store responsibilities.
    pub fn verify(&self) -> Result<()> {
        self.verify_for(CROSSLATCH_V1)
    }

    /// Verify proof of possession for the server-selected application domain.
    ///
    /// # Errors
    /// Rejects malformed or differently signed enrollment data. Does not
    /// authenticate a ticket, consume it, enroll a device or grant anything.
    pub fn verify_for(&self, domain: SigningDomain) -> Result<()> {
        ensure!(
            self.token.len() == 64
                && !self.name.trim().is_empty()
                && self.name.len() <= 120
                && !self.name.contains('\n'),
            "invalid enrollment"
        );
        verify_signature(
            &self.public_key,
            &self.signature,
            &self.signing_bytes_for(domain),
        )
    }
}

/// Signed request whose payload is an exact string, not reconstructed JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedRequest {
    /// Store-assigned paired device identity.
    pub device_id: String,
    /// Unix time with a maximum tolerated skew of 60 seconds.
    pub timestamp: i64,
    /// Unique 16–120 byte ASCII alphanumeric/hyphen/underscore nonce.
    pub nonce: String,
    /// Exact application payload; parse only after authenticating the proof.
    pub payload: String,
    /// Base64 Ed25519 signature.
    pub signature: String,
}

impl SignedRequest {
    /// Existing `CrossLatch` v1 signing bytes, preserved for native clients.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        self.signing_bytes_for(CROSSLATCH_V1)
    }

    /// Domain-separated signing bytes, independent of envelope JSON formatting.
    #[must_use]
    pub fn signing_bytes_for(&self, domain: SigningDomain) -> Vec<u8> {
        format!(
            "{}\n{}\n{}\n{}\n{}",
            domain.request, self.device_id, self.timestamp, self.nonce, self.payload
        )
        .into_bytes()
    }

    /// Validate framing, clock window and signature for a trusted stored key.
    ///
    /// # Errors
    /// Rejects malformed/bounded inputs, clock skew and forged/cross-domain
    /// signatures. Does NOT check device enrollment/revocation, nonce uniqueness
    /// or grants. Persist the nonce and authorize before dispatching anything.
    pub fn verify_for(&self, domain: SigningDomain, public_key: &str, now: i64) -> Result<()> {
        ensure!(
            !self.device_id.is_empty()
                && self.device_id.len() <= 256
                && !self.device_id.contains(['\n', '\r'])
                && self.payload.len() <= MAX_PAYLOAD_BYTES,
            "invalid request bounds"
        );
        ensure!(
            (16..=120).contains(&self.nonce.len())
                && self
                    .nonce
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "invalid nonce"
        );
        ensure!(
            now.abs_diff(self.timestamp) <= 60,
            "request expired; check device clock"
        );
        verify_signature(public_key, &self.signature, &self.signing_bytes_for(domain))
    }
}

fn verify_signature(public_key: &str, signature: &str, bytes: &[u8]) -> Result<()> {
    ensure!(
        public_key.len() <= 64 && signature.len() <= 128,
        "invalid proof bounds"
    );
    let key: [u8; 32] = STANDARD
        .decode(public_key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid public key length"))?;
    let key = VerifyingKey::from_bytes(&key)?;
    let signature = Signature::from_slice(&STANDARD.decode(signature)?)?;
    key.verify_strict(bytes, &signature)
        .context("invalid signature")
}
