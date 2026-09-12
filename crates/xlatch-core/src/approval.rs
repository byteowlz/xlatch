//! Exact, phone-signed capability activation and additive device grants.
use crate::{
    capability::Manifest,
    enrollment,
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Approver-only operations carried by an authenticated device RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalRequest {
    /// List all registrations and active devices for review.
    Catalog,
    /// Freeze the chosen manifest and additive grants into a ten-minute review.
    Prepare {
        /// Capability identifier.
        capability_id: String,
        /// Expected current revision.
        revision: String,
        /// Devices to grant; other devices' grants are unchanged.
        devices: Vec<String>,
    },
    /// Consume a signed review once; rejection never changes permissions.
    Decide {
        /// Review identity.
        id: String,
        /// Decision bound into the signature.
        approve: bool,
        /// Base64 DER ECDSA-SHA256 signature.
        signature: String,
    },
}

/// Device identity shown and bound into the permission decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantTarget {
    /// Stable device identifier.
    pub id: String,
    /// Untrusted human-readable label.
    pub name: String,
    /// Base64 Ed25519 request key.
    pub public_key: String,
}

/// Immutable JSON review; sign its exact bytes rather than reserializing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReview {
    /// Unique single-use challenge.
    pub id: String,
    /// Server identity.
    pub server_id: String,
    /// Approval protocol policy version.
    pub policy_version: u8,
    /// Approver who requested this review.
    pub approver_id: String,
    /// Full execution and input/output contract.
    pub manifest: Manifest,
    /// Exact current manifest digest.
    pub revision: String,
    /// Additive grants, each to this exact revision.
    pub devices: Vec<GrantTarget>,
    /// Unix expiry.
    pub expires_at: i64,
}

/// Domain separation prevents enrollment signatures from activating capabilities.
#[must_use]
pub fn decision_bytes(payload: &str, approve: bool) -> Vec<u8> {
    let decision = if approve { "approve" } else { "reject" };
    format!("xlatch.capability.decision.v1\n{decision}\n{payload}").into_bytes()
}

fn targets(conn: &Connection) -> Result<Vec<GrantTarget>> {
    let mut stmt = conn.prepare("SELECT id,name,public_key FROM devices WHERE revoked=0 AND enrollment_status='active' ORDER BY id")?;
    Ok(stmt
        .query_map([], |r| {
            Ok(GrantTarget {
                id: r.get(0)?,
                name: r.get(1)?,
                public_key: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

impl Store {
    fn prepare_approval(
        &mut self,
        owner: &str,
        id: &str,
        revision: &str,
        devices: &[String],
    ) -> Result<Value> {
        ensure!(devices.len() <= 100, "select at most 100 devices");
        let capability = self.capability(id)?;
        ensure!(
            capability.revision == revision,
            "revision changed; review again"
        );
        capability.manifest.validate()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Rechecked inside the write transaction to close registration/approval races.
        let current: String =
            tx.query_row("SELECT revision FROM capabilities WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        ensure!(current == revision, "revision changed; review again");
        let available = targets(&tx)?;
        let mut selected = Vec::new();
        for device in devices {
            ensure!(
                !selected.iter().any(|t: &GrantTarget| t.id == *device),
                "duplicate grant target"
            );
            selected.push(
                available
                    .iter()
                    .find(|t| t.id == *device)
                    .context("grant target is not active")?
                    .clone(),
            );
        }
        // One outstanding review per approver; another prepare deliberately invalidates it.
        tx.execute("DELETE FROM capability_approvals WHERE decision IS NULL AND (owner=?1 OR expires_at<?2)", params![owner, now()])?;
        let review = ApprovalReview {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: tx.query_row(
                "SELECT server_id FROM enrollment_policy WHERE singleton=1",
                [],
                |r| r.get(0),
            )?,
            policy_version: 1,
            approver_id: owner.into(),
            manifest: capability.manifest,
            revision: revision.into(),
            devices: selected,
            expires_at: now() + 600,
        };
        let payload = serde_json::to_string(&review)?;
        tx.execute(
            "INSERT INTO capability_approvals(id,owner,payload,expires_at) VALUES(?1,?2,?3,?4)",
            params![review.id, owner, payload, review.expires_at],
        )?;
        tx.commit()?;
        Ok(json!(payload))
    }

    fn decide_approval(
        &mut self,
        owner: &str,
        id: &str,
        approve: bool,
        signature: &str,
    ) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure!(
            enrollment::is_approver(&tx, owner)?,
            "an enrolled approver is required"
        );
        let payload: String = tx.query_row("SELECT payload FROM capability_approvals WHERE id=?1 AND owner=?2 AND decision IS NULL AND expires_at>=?3", params![id,owner,now()], |r| r.get(0)).context("approval expired or already decided; review again")?;
        enrollment::verify_approver(&tx, owner, signature, &decision_bytes(&payload, approve))?;
        let review: ApprovalReview = serde_json::from_str(&payload)?;
        let server: String = tx.query_row(
            "SELECT server_id FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            review.server_id == server
                && review.policy_version == 1
                && review.approver_id == owner
                && review.id == id
                && review.expires_at >= now(),
            "approval context changed"
        );
        if approve {
            activate(&tx, &review)?;
        }
        let decision = if approve { "approved" } else { "rejected" };
        tx.execute(
            "UPDATE capability_approvals SET decision=?2,signature=?3,decided_at=?4 WHERE id=?1",
            params![id, decision, signature, now()],
        )?;
        tx.commit()?;
        Ok(json!({"id":id,"status":decision}))
    }
}

fn activate(conn: &Connection, review: &ApprovalReview) -> Result<()> {
    review.manifest.validate()?;
    ensure!(
        review.manifest.revision()? == review.revision,
        "manifest digest mismatch"
    );
    let (body, revision): (String, String) = conn.query_row(
        "SELECT manifest,revision FROM capabilities WHERE id=?1",
        [&review.manifest.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        revision == review.revision && serde_json::from_str::<Manifest>(&body)? == review.manifest,
        "capability changed; review again"
    );
    // Do not inspect agent-controlled files with broker privileges. The executor checks
    // executable hashes and destination access under its own identity before each job.
    conn.execute(
        "UPDATE capabilities SET status='active' WHERE id=?1 AND revision=?2",
        params![review.manifest.id, review.revision],
    )?;
    for target in &review.devices {
        let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1 AND public_key=?2 AND name=?3 AND revoked=0 AND enrollment_status='active')", params![target.id,target.public_key,target.name], |r| r.get(0))?;
        ensure!(active, "grant target changed; review again");
        conn.execute("INSERT INTO grants(device_id,capability_id,revision) VALUES(?1,?2,?3) ON CONFLICT(device_id,capability_id) DO UPDATE SET revision=excluded.revision", params![target.id,review.manifest.id,review.revision])?;
    }
    Ok(())
}

/// Dispatch under an active approver; local operator authority is insufficient.
///
/// # Errors
/// Rejects unauthorized callers, stale reviews, invalid signatures and storage failures.
pub fn dispatch(store: &mut Store, owner: &str, request: ApprovalRequest) -> Result<Value> {
    ensure!(
        owner != "local" && enrollment::is_approver(&store.conn, owner)?,
        "an enrolled approver is required"
    );
    match request {
        ApprovalRequest::Catalog => {
            Ok(json!({"capabilities":store.discover("local")?,"devices":targets(&store.conn)?}))
        }
        ApprovalRequest::Prepare {
            capability_id,
            revision,
            devices,
        } => store.prepare_approval(owner, &capability_id, &revision, &devices),
        ApprovalRequest::Decide {
            id,
            approve,
            signature,
        } => store.decide_approval(owner, &id, approve, &signature),
    }
}
