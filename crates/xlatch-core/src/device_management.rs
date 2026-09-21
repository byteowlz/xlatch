//! Device aliases are labels, never credentials. Protected changes require signed reviews.
use crate::{
    enrollment::{is_approver, verify_approver},
    store::{Store, now},
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Exact device mutation displayed by an approver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    /// Set a separate label without changing the enrollment identity.
    Alias {
        /// None restores the original name.
        alias: Option<String>,
    },
    /// Revoke access, retaining historical references.
    Remove,
}
/// Approver-only management operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeviceRequest {
    /// Devices and pending, expiring management reviews.
    List,
    /// Freeze a proposed change for biometric approval.
    Prepare {
        /// Stable target identity.
        id: String,
        /// Requested mutation.
        change: Change,
    },
    /// Consume a signed review once.
    Decide {
        /// Review ID.
        id: String,
        /// Approve or reject.
        approve: bool,
        /// DER P-256 signature in base64.
        signature: String,
    },
}
#[derive(Serialize, Deserialize)]
struct Review {
    id: String,
    server_id: String,
    device_id: String,
    name: String,
    alias: Option<String>,
    public_key: String,
    change: Change,
    expires_at: i64,
}
/// Domain-separated signature frame, using exact server-provided JSON.
#[must_use]
pub fn decision_bytes(payload: &str, approve: bool) -> Vec<u8> {
    format!(
        "xlatch.device.decision.v1\n{}\n{payload}",
        if approve { "approve" } else { "reject" }
    )
    .into_bytes()
}
fn validate(change: &Change) -> Result<()> {
    if let Change::Alias { alias: Some(alias) } = change {
        ensure!(
            !alias.trim().is_empty() && alias.len() <= 128 && !alias.chars().any(char::is_control),
            "alias must contain 1–128 bytes without control characters"
        );
    }
    Ok(())
}
fn apply(conn: &Connection, id: &str, change: &Change, approver: Option<&str>) -> Result<()> {
    match change {
        Change::Alias { alias } => {
            ensure!(
                conn.execute(
                    "UPDATE devices SET alias=?2 WHERE id=?1 AND revoked=0",
                    params![id, alias]
                )? == 1,
                "device not found or revoked"
            );
        }
        Change::Remove => {
            if is_approver(conn, id)? {
                ensure!(
                    approver.is_some_and(|owner| owner != id),
                    "removing an approver requires a different approver's signature"
                );
            }
            ensure!(
                conn.execute(
                    "UPDATE devices SET revoked=1 WHERE id=?1 AND revoked=0",
                    [id]
                )? == 1,
                "device not found or already removed"
            );
            conn.execute("DELETE FROM approvers WHERE device_id=?1", [id])?;
            conn.execute("DELETE FROM grants WHERE device_id=?1", [id])?;
            conn.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'cancelled' FROM jobs WHERE owner=?1 AND status IN ('queued','running')",[id])?;
            conn.execute("UPDATE jobs SET status='cancelled' WHERE owner=?1 AND status IN ('queued','running')",[id])?;
        }
    }
    // Concurrent reviews of this identity must be prepared again after any change.
    conn.execute(
        "DELETE FROM device_reviews WHERE json_extract(payload,'$.device_id')=?1",
        [id],
    )?;
    Ok(())
}
impl Store {
    /// Apply a trusted local change, or queue it without authority in protected mode.
    /// # Errors
    /// Rejects invalid labels, unknown devices and unsigned approver removal.
    pub fn change_device_local(
        &mut self,
        id: &str,
        change: Change,
        protected: bool,
    ) -> Result<Value> {
        validate(&change)?;
        if protected {
            let payload = self.prepare_device_change(id, change)?;
            return Ok(
                json!({"pending":true,"message":"Review this change on an approver client under Devices.","payload":payload}),
            );
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        apply(&tx, id, &change, None)?;
        tx.commit()?;
        Ok(json!({"device_id":id,"applied":true}))
    }
    fn prepare_device_change(&mut self, id: &str, change: Change) -> Result<String> {
        validate(&change)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM device_reviews WHERE expires_at<?1", [now()])?;
        let count: i64 = tx.query_row("SELECT count(*) FROM device_reviews", [], |r| r.get(0))?;
        ensure!(count < 100, "too many pending device reviews");
        let (name, alias, key): (String, Option<String>, String) = tx.query_row(
            "SELECT name,alias,public_key FROM devices WHERE id=?1 AND revoked=0",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let server_id = tx.query_row(
            "SELECT server_id FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let review = Review {
            id: uuid::Uuid::new_v4().to_string(),
            server_id,
            device_id: id.into(),
            name,
            alias,
            public_key: key,
            change,
            expires_at: now() + 600,
        };
        let payload = serde_json::to_string(&review)?;
        tx.execute(
            "INSERT INTO device_reviews VALUES(?1,?2,?3)",
            params![review.id, payload, review.expires_at],
        )?;
        tx.commit()?;
        Ok(payload)
    }
    fn decide_device_change(
        &mut self,
        owner: &str,
        id: &str,
        approve: bool,
        signature: &str,
    ) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let payload: String = tx.query_row(
            "SELECT payload FROM device_reviews WHERE id=?1 AND expires_at>=?2",
            params![id, now()],
            |r| r.get(0),
        )?;
        verify_approver(&tx, owner, signature, &decision_bytes(&payload, approve))?;
        let review: Review = serde_json::from_str(&payload)?;
        let valid:bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM devices d JOIN enrollment_policy p ON p.singleton=1 WHERE d.id=?1 AND d.public_key=?2 AND d.name=?3 AND d.alias IS ?4 AND d.revoked=0 AND p.server_id=?5)",params![review.device_id,review.public_key,review.name,review.alias,review.server_id],|r|r.get(0))?;
        ensure!(valid && review.id == id, "device review context changed");
        if approve {
            apply(&tx, &review.device_id, &review.change, Some(owner))?;
        }
        tx.execute("DELETE FROM device_reviews WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(json!({"approved":approve}))
    }
}
/// Dispatch under an authenticated approver; ordinary clients cannot enumerate devices.
/// # Errors
/// Rejects missing authority, forged signatures, expiry and replay.
pub fn dispatch(store: &mut Store, owner: &str, request: DeviceRequest) -> Result<Value> {
    ensure!(
        owner != "local" && is_approver(&store.conn, owner)?,
        "an enrolled approver is required"
    );
    match request {
        DeviceRequest::List => {
            let mut stmt = store.conn.prepare(
                "SELECT payload FROM device_reviews WHERE expires_at>=?1 ORDER BY expires_at",
            )?;
            let pending = stmt
                .query_map([now()], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({"devices":store.devices()?,"pending":pending}))
        }
        DeviceRequest::Prepare { id, change } => {
            Ok(json!(store.prepare_device_change(&id, change)?))
        }
        DeviceRequest::Decide {
            id,
            approve,
            signature,
        } => store.decide_device_change(owner, &id, approve, &signature),
    }
}
