//! Backup approvers: possession of a client key alone never grants approval authority.
use crate::{
    enrollment::{is_approver, verify_approver},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Operations available only to authenticated, active devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryRequest {
    /// An active client proposes its own biometric approval key.
    Propose {
        /// Base64 SEC1 P-256 public key.
        public_key: String,
        /// Base64 DER signature of the domain-separated frame.
        signature: String,
    },
    /// Read own proposal, or all proposals and approvers when already an approver.
    Status,
    /// An approver prepares removal of a different approver.
    Remove {
        /// Existing approver device to revoke.
        device_id: String,
    },
    /// Approve or reject an exact one-time review with an existing approval key.
    Decide {
        /// One-time review ID.
        id: String,
        /// Decision included in the signature.
        approve: bool,
        /// Base64 DER signature of the domain-separated frame.
        signature: String,
    },
}
/// Exact serialized review signed by an existing, different approver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    /// Random review identity.
    pub id: String,
    /// Server trust identity.
    pub server_id: String,
    /// Operation: add or remove.
    pub operation: String,
    /// Target device identity.
    pub device_id: String,
    /// Untrusted display label.
    pub name: String,
    /// Target request authentication key.
    pub request_key: String,
    /// Proposed or existing approval public key.
    pub public_key: String,
    /// Review expiry.
    pub expires_at: i64,
}
/// Proof that the candidate possesses the proposed key; does not grant authority.
#[must_use]
pub fn proof_bytes(server: &str, device: &str, key: &str) -> Vec<u8> {
    format!("xlatch.approver.proof.v1\n{server}\n{device}\n{key}").into_bytes()
}
/// Signature frame distinct from ordinary device and capability approvals.
#[must_use]
pub fn decision_bytes(payload: &str, approve: bool) -> Vec<u8> {
    let decision = if approve { "approve" } else { "reject" };
    format!("xlatch.approver.decision.v1\n{decision}\n{payload}").into_bytes()
}
impl Store {
    fn prepare_approver(&self, device: &str, public_key: &str, operation: &str) -> Result<Value> {
        let (name,request_key): (String,String) = self.conn.query_row("SELECT name,public_key FROM devices WHERE id=?1 AND revoked=0 AND enrollment_status='active'", [device], |r| Ok((r.get(0)?,r.get(1)?)))?;
        let review = Review {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: self.server_identity()?,
            operation: operation.into(),
            device_id: device.into(),
            name,
            request_key,
            public_key: public_key.into(),
            expires_at: now() + 600,
        };
        let payload = serde_json::to_string(&review)?;
        self.conn.execute(
            "DELETE FROM approver_reviews WHERE device_id=?1 OR expires_at<?2",
            params![device, now()],
        )?;
        self.conn.execute(
            "INSERT INTO approver_reviews VALUES(?1,?2,?3,?4)",
            params![review.id, device, payload, review.expires_at],
        )?;
        Ok(json!(payload))
    }
    fn approver_status(&self, owner: &str) -> Result<Value> {
        let approver = is_approver(&self.conn, owner)?;
        let mut stmt = self.conn.prepare("SELECT payload FROM approver_reviews WHERE expires_at>=?1 AND (?2 OR device_id=?3) ORDER BY id")?;
        let pending = stmt
            .query_map(params![now(), approver, owner], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let devices = if approver {
            let mut stmt = self.conn.prepare("SELECT d.id,d.name,a.public_key FROM approvers a JOIN devices d ON d.id=a.device_id WHERE d.revoked=0 AND d.enrollment_status='active' ORDER BY d.id")?;
            stmt.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"public_key":r.get::<_,String>(2)?})))?.collect::<Result<Vec<_>,_>>()?
        } else {
            Vec::new()
        };
        Ok(json!({"pending":pending,"approvers":devices}))
    }
    fn decide_approver(
        &mut self,
        owner: &str,
        id: &str,
        approve: bool,
        signature: &str,
    ) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let payload: String = tx
            .query_row(
                "SELECT payload FROM approver_reviews WHERE id=?1 AND expires_at>=?2",
                params![id, now()],
                |r| r.get(0),
            )
            .context("approver review expired or consumed")?;
        verify_approver(&tx, owner, signature, &decision_bytes(&payload, approve))?;
        let review: Review = serde_json::from_str(&payload)?;
        ensure!(
            review.device_id != owner,
            "another approver must authorize this change"
        );
        let server: String = tx.query_row(
            "SELECT server_id FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            server == review.server_id && review.id == id,
            "review context changed"
        );
        let key: String = tx.query_row("SELECT public_key FROM devices WHERE id=?1 AND revoked=0 AND enrollment_status='active'",[&review.device_id],|r|r.get(0))?;
        ensure!(key == review.request_key, "device identity changed");
        if approve {
            match review.operation.as_str() {
                "add" => {
                    tx.execute(
                        "INSERT INTO approvers VALUES(?1,?2)",
                        params![review.device_id, review.public_key],
                    )?;
                }
                "remove" => {
                    ensure!(
                        tx.execute(
                            "DELETE FROM approvers WHERE device_id=?1 AND public_key=?2",
                            params![review.device_id, review.public_key]
                        )? == 1,
                        "approver changed"
                    );
                    tx.execute(
                        "UPDATE devices SET revoked=1 WHERE id=?1",
                        [&review.device_id],
                    )?;
                    tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'cancelled' FROM jobs WHERE owner=?1 AND status IN ('queued','running')",[&review.device_id])?;
                    tx.execute("UPDATE jobs SET status='cancelled' WHERE owner=?1 AND status IN ('queued','running')",[&review.device_id])?;
                }
                _ => anyhow::bail!("unsupported approver operation"),
            }
        }
        tx.execute("DELETE FROM approver_reviews WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(json!({"approved":approve}))
    }
}
/// Dispatch after ordinary device authentication, with separate biometric authorization.
/// # Errors
/// Rejects client self-promotion, forged approvals, stale reviews and local impersonation.
pub fn dispatch(store: &mut Store, owner: &str, request: RecoveryRequest) -> Result<Value> {
    ensure!(
        owner != "local",
        "recovery requires an authenticated device"
    );
    match request {
        RecoveryRequest::Status => store.approver_status(owner),
        RecoveryRequest::Propose {
            public_key,
            signature,
        } => {
            ensure!(!is_approver(&store.conn, owner)?, "already an approver");
            crate::enrollment::verify(
                &public_key,
                &signature,
                &proof_bytes(&store.server_identity()?, owner, &public_key),
            )?;
            store.prepare_approver(owner, &public_key, "add")
        }
        RecoveryRequest::Remove { device_id } => {
            ensure!(
                is_approver(&store.conn, owner)? && owner != device_id,
                "another approver is required"
            );
            let key: String = store.conn.query_row(
                "SELECT public_key FROM approvers WHERE device_id=?1",
                [&device_id],
                |r| r.get(0),
            )?;
            store.prepare_approver(&device_id, &key, "remove")
        }
        RecoveryRequest::Decide {
            id,
            approve,
            signature,
        } => store.decide_approver(owner, &id, approve, &signature),
    }
}
