//! Phone-held approval keys, separate from ordinary device request authentication.

use crate::{
    auth::Device,
    capability::{Request, digest},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Enrollment operations inside the ordinary authenticated RPC envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnrollmentRequest {
    /// Manage backup approvers using a separately signed review.
    Recovery {
        /// Recovery operation.
        request: crate::recovery::RecoveryRequest,
    },
    /// Own enrollment status and server policy; also accessible to pending devices.
    Status,
    /// Establish the first approval key using an operator-issued bootstrap secret.
    Enable {
        /// One-time secret scoped to this device.
        token: String,
        /// Base64 uncompressed SEC1 P-256 public key.
        public_key: String,
        /// Base64 DER ECDSA-SHA256 signature of the enable frame.
        signature: String,
    },
    /// List live requests; only an enrolled approver may review them.
    Pending,
    /// Decide the exact immutable request; activation never makes an approver.
    Decide {
        /// Candidate device identity.
        id: String,
        /// Approve or reject; included in the signed frame.
        approve: bool,
        /// Base64 DER ECDSA-SHA256 signature of the decision frame.
        signature: String,
    },
}

/// Immutable review payload. The exact JSON string is signed, not reserialized JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentReview {
    /// Database identity; a reset must create a new identity and TLS certificate.
    pub server_id: String,
    /// Version of the policy under which this request was created.
    pub policy_version: u8,
    /// Candidate identity, selected by the server.
    pub device_id: String,
    /// Untrusted candidate label, displayed alongside its key fingerprint.
    pub name: String,
    /// Candidate's Ed25519 request key.
    pub public_key: String,
    /// Exact capability identifiers and revisions.
    pub grants: Vec<(String, String)>,
    /// One-time random challenge.
    pub nonce: String,
    /// Unix expiry, ten minutes after the QR scan.
    pub expires_at: i64,
}

/// Derive an enrollment decision frame shared with native clients.
#[must_use]
pub fn decision_bytes(payload: &str, approve: bool) -> Vec<u8> {
    let decision = if approve { "approve" } else { "reject" };
    format!("xlatch.enrollment.decision.v1\n{decision}\n{payload}").into_bytes()
}

/// Bind first-approver registration to the server, device, secret and new key.
#[must_use]
pub fn enable_bytes(server: &str, device: &str, token: &str, key: &str) -> Vec<u8> {
    format!("xlatch.enrollment.enable.v1\n{server}\n{device}\n{token}\n{key}").into_bytes()
}

pub(crate) fn verify(key: &str, signature: &str, bytes: &[u8]) -> Result<()> {
    let key = STANDARD.decode(key)?;
    ensure!(
        key.len() == 65 && key.first() == Some(&4),
        "expected uncompressed P-256 key"
    );
    let key = VerifyingKey::from_sec1_bytes(&key)?;
    let signature = Signature::from_der(&STANDARD.decode(signature)?)?;
    key.verify(bytes, &signature)
        .context("invalid approval signature")
}

pub(crate) fn verify_approver(
    conn: &Connection,
    owner: &str,
    signature: &str,
    bytes: &[u8],
) -> Result<()> {
    ensure!(
        is_approver(conn, owner)?,
        "an enrolled approver is required"
    );
    let key: String = conn.query_row(
        "SELECT public_key FROM approvers WHERE device_id=?1",
        [owner],
        |r| r.get(0),
    )?;
    verify(&key, signature, bytes)
}

pub(crate) fn is_approver(conn: &Connection, owner: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM approvers a JOIN devices d ON d.id=a.device_id WHERE d.id=?1 AND d.revoked=0 AND d.enrollment_status='active')", [owner], |r| r.get(0))?)
}

fn server_id(conn: &Connection) -> Result<String> {
    Ok(conn.query_row(
        "SELECT server_id FROM enrollment_policy WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}

pub(crate) fn queue(
    conn: &Connection,
    device: &Device,
    key: &str,
    grants: &[(String, String)],
) -> Result<()> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM pending_enrollments WHERE decision IS NULL AND expires_at>=?1",
        [now()],
        |r| r.get(0),
    )?;
    ensure!(count < 100, "too many pending enrollments");
    let review = EnrollmentReview {
        server_id: server_id(conn)?,
        policy_version: 1,
        device_id: device.id.clone(),
        name: device.name.clone(),
        public_key: key.into(),
        grants: grants.to_vec(),
        nonce: uuid::Uuid::new_v4().to_string(),
        expires_at: now() + 600,
    };
    conn.execute(
        "INSERT INTO pending_enrollments(device_id,payload,expires_at) VALUES(?1,?2,?3)",
        params![
            device.id,
            serde_json::to_string(&review)?,
            review.expires_at
        ],
    )?;
    Ok(())
}

impl Store {
    /// Stable server identity for an explicit certificate migration.
    ///
    /// # Errors
    /// Returns storage errors.
    pub fn server_identity(&self) -> Result<String> {
        server_id(&self.conn)
    }

    /// Pending devices may poll only their own status; local RPC cannot impersonate phones.
    ///
    /// # Errors
    /// Rejects unknown, revoked, or not-yet-approved devices.
    pub fn authorize_request(&self, owner: &str, request: &Request) -> Result<()> {
        if owner == "local" {
            return Ok(());
        }
        let state: String = self
            .conn
            .query_row(
                "SELECT enrollment_status FROM devices WHERE id=?1 AND revoked=0",
                [owner],
                |r| r.get(0),
            )
            .context("unknown or revoked device")?;
        ensure!(
            state == "active"
                || matches!(
                    request,
                    Request::Enrollment {
                        request: EnrollmentRequest::Status
                    }
                ),
            "device enrollment is not approved"
        );
        Ok(())
    }

    /// Issue an explicit first-approver bootstrap secret, never an unlock for enabled policy.
    ///
    /// # Errors
    /// Rejects enabled protection and inactive devices.
    pub fn enrollment_bootstrap(&mut self, device: &str) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let enabled: bool = tx.query_row(
            "SELECT enabled FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(!enabled, "phone-approved enrollment is already enabled");
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1 AND revoked=0 AND enrollment_status='active')", [device], |r| r.get(0))?;
        ensure!(active, "select an existing active device");
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expiry = now() + 300;
        tx.execute("DELETE FROM enrollment_bootstrap", [])?;
        tx.execute(
            "INSERT INTO enrollment_bootstrap VALUES(?1,?2,?3)",
            params![device, digest(token.as_bytes()), expiry],
        )?;
        tx.commit()?;
        Ok(
            json!({"purpose":"xlatch.approval-bootstrap","server_id":server_id(&self.conn)?,"token":token,"expires_at":expiry,"device_id":device}),
        )
    }

    fn enrollment_status(&self, owner: &str) -> Result<Value> {
        let state: String = self.conn.query_row(
            "SELECT enrollment_status FROM devices WHERE id=?1 AND revoked=0",
            [owner],
            |r| r.get(0),
        )?;
        let enabled: bool = self.conn.query_row(
            "SELECT enabled FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let pending: Option<String> = self
            .conn
            .query_row(
                "SELECT payload FROM pending_enrollments WHERE device_id=?1",
                [owner],
                |r| r.get(0),
            )
            .optional()?;
        let expired = pending
            .as_ref()
            .map(|s| serde_json::from_str::<EnrollmentReview>(s).map(|r| r.expires_at < now()))
            .transpose()?
            .unwrap_or(false);
        Ok(
            json!({"server_id":server_id(&self.conn)?,"enabled":enabled,"is_approver":is_approver(&self.conn,owner)?,"device_status":if state == "pending" && expired {"expired"} else {&state},"pending_payload":pending}),
        )
    }

    fn enable_enrollment(
        &mut self,
        owner: &str,
        token: &str,
        public_key: &str,
        signature: &str,
    ) -> Result<Value> {
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid bootstrap token"
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let eligible: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM enrollment_bootstrap b JOIN devices d ON d.id=b.device_id JOIN enrollment_policy p ON p.singleton=1 WHERE b.device_id=?1 AND b.token_hash=?2 AND b.expires_at>=?3 AND d.revoked=0 AND d.enrollment_status='active' AND p.enabled=0)", params![owner,digest(token.as_bytes()),now()], |r| r.get(0))?;
        ensure!(
            eligible,
            "invalid, expired or consumed first-approver bootstrap"
        );
        verify(
            public_key,
            signature,
            &enable_bytes(&server_id(&tx)?, owner, token, public_key),
        )?;
        tx.execute(
            "INSERT INTO approvers VALUES(?1,?2)",
            params![owner, public_key],
        )?;
        tx.execute(
            "UPDATE enrollment_policy SET enabled=1 WHERE singleton=1",
            [],
        )?;
        tx.execute("DELETE FROM enrollment_bootstrap", [])?;
        // Tickets issued before protection was enabled must not retain bootstrap authority.
        tx.execute("DELETE FROM tickets", [])?;
        tx.commit()?;
        self.enrollment_status(owner)
    }

    fn pending_enrollments(&self, owner: &str) -> Result<Value> {
        ensure!(
            is_approver(&self.conn, owner)?,
            "an enrolled approver is required"
        );
        let mut stmt = self.conn.prepare("SELECT p.payload FROM pending_enrollments p JOIN devices d ON d.id=p.device_id WHERE p.decision IS NULL AND p.expires_at>=?1 AND d.revoked=0 AND d.enrollment_status='pending' ORDER BY p.expires_at")?;
        let payloads = stmt
            .query_map([now()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::to_value(payloads)?)
    }

    fn decide_enrollment(
        &mut self,
        owner: &str,
        id: &str,
        approve: bool,
        signature: &str,
    ) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(is_approver(&tx, owner)?, "an enrolled approver is required");
        let payload: String = tx.query_row("SELECT p.payload FROM pending_enrollments p JOIN devices d ON d.id=p.device_id WHERE p.device_id=?1 AND p.decision IS NULL AND p.expires_at>=?2 AND d.revoked=0 AND d.enrollment_status='pending'", params![id,now()], |r| r.get(0)).context("enrollment expired or already decided")?;
        verify_approver(&tx, owner, signature, &decision_bytes(&payload, approve))?;
        let review: EnrollmentReview = serde_json::from_str(&payload)?;
        ensure!(
            review.server_id == server_id(&tx)?
                && review.policy_version == 1
                && review.device_id == id,
            "enrollment context changed"
        );
        if approve {
            activate(&tx, &review)?;
        }
        let state = if approve { "active" } else { "rejected" };
        tx.execute(
            "UPDATE devices SET enrollment_status=?2 WHERE id=?1",
            params![id, state],
        )?;
        tx.execute(
            "UPDATE pending_enrollments SET decision=?2 WHERE device_id=?1",
            params![id, state],
        )?;
        tx.commit()?;
        Ok(json!({"device_id":id,"status":state}))
    }
}

fn activate(conn: &Connection, review: &EnrollmentReview) -> Result<()> {
    let unchanged: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1 AND public_key=?2)",
        params![review.device_id, review.public_key],
        |r| r.get(0),
    )?;
    ensure!(unchanged, "candidate key changed");
    for (id, revision) in &review.grants {
        let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM capabilities WHERE id=?1 AND revision=?2 AND status='active')", params![id,revision], |r| r.get(0))?;
        ensure!(valid, "capability changed; request a new pairing code");
        conn.execute(
            "INSERT INTO grants VALUES(?1,?2,?3)",
            params![review.device_id, id, revision],
        )?;
    }
    Ok(())
}

/// Dispatch enrollment under an authenticated device, never the local operator identity.
///
/// # Errors
/// Returns authorization, signature, expiry, and persistence errors.
pub fn dispatch(store: &mut Store, owner: &str, request: EnrollmentRequest) -> Result<Value> {
    ensure!(
        owner != "local",
        "enrollment approval requires an authenticated phone"
    );
    match request {
        EnrollmentRequest::Recovery { request } => crate::recovery::dispatch(store, owner, request),
        EnrollmentRequest::Status => store.enrollment_status(owner),
        EnrollmentRequest::Enable {
            token,
            public_key,
            signature,
        } => store.enable_enrollment(owner, &token, &public_key, &signature),
        EnrollmentRequest::Pending => store.pending_enrollments(owner),
        EnrollmentRequest::Decide {
            id,
            approve,
            signature,
        } => store.decide_enrollment(owner, &id, approve, &signature),
    }
}

impl Store {
    /// Verify a phone-displayed trust anchor before importing user-owned approval state.
    /// Invalidate every other device, ticket and pending enrollment during the migration.
    ///
    /// # Errors
    /// Rejects missing/multiple approvers, a changed identity/key, or an incorrect anchor.
    pub fn prepare_protected_migration(&mut self, expected_anchor: &str) -> Result<()> {
        ensure!(
            expected_anchor.len() == 64 && expected_anchor.bytes().all(|b| b.is_ascii_hexdigit()),
            "copy the protected-installation fingerprint from the approver phone"
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM approvers", [], |r| r.get(0))?;
        ensure!(
            count == 1,
            "migration requires exactly one explicitly verified approver"
        );
        let (id,key,approval): (String,String,String) = tx.query_row("SELECT d.id,d.public_key,a.public_key FROM devices d JOIN approvers a ON a.device_id=d.id WHERE d.revoked=0 AND d.enrollment_status='active'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let frame = format!(
            "xlatch.protected.anchor.v1\n{}\n{id}\n{key}\n{approval}",
            server_id(&tx)?
        );
        ensure!(
            digest(frame.as_bytes()) == expected_anchor.to_ascii_lowercase(),
            "approver fingerprint does not match the phone; do not migrate this state"
        );
        tx.execute("UPDATE devices SET revoked=1 WHERE id<>?1", [&id])?;
        tx.execute("DELETE FROM grants WHERE device_id<>?1", [&id])?;
        tx.execute("DELETE FROM tickets", [])?;
        tx.execute("DELETE FROM enrollment_bootstrap", [])?;
        tx.execute("DELETE FROM pending_enrollments", [])?;
        tx.execute("DELETE FROM capability_approvals", [])?;
        tx.execute("DELETE FROM nonces", [])?;
        tx.commit()?;
        Ok(())
    }
}
