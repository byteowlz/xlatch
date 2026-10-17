//! QR enrollment and replay-resistant Ed25519 device authentication.

use crate::{
    capability::{Request, digest},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use xlatch_wire_auth::CROSSLATCH_V1;
pub use xlatch_wire_auth::{Enrollment, SignedRequest};

/// Out-of-band bootstrap, displayed only to the local operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingTicket {
    /// Protocol version.
    pub version: u8,
    /// HTTPS server origin.
    pub url: String,
    /// Alternate origins sharing the same pinned certificate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// Lowercase SHA-256 of the server's DER certificate.
    pub pin: String,
    /// Single-use, random enrollment secret.
    pub token: String,
    /// Unix expiry time.
    pub expires_at: i64,
}

/// Authenticated device metadata, with no private keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// Server-assigned identity.
    pub id: String,
    /// Name selected on enrollment.
    pub name: String,
    /// Operator-selected display label; never an identity credential.
    pub alias: Option<String>,
    /// Whether access has been revoked.
    pub revoked: bool,
    /// Active or pending; pending devices cannot invoke capabilities.
    pub enrollment_status: String,
}

impl Store {
    /// Create a five-minute enrollment ticket granting only specified active revisions.
    ///
    /// # Errors
    /// Rejects non-HTTPS origins and unapproved capabilities.
    pub fn pairing_ticket(
        &self,
        url: String,
        pin: String,
        capabilities: &[String],
    ) -> Result<PairingTicket> {
        let parsed = url::Url::parse(&url)?;
        ensure!(
            parsed.scheme() == "https"
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.path() == "/",
            "pairing requires an HTTPS origin"
        );
        ensure!(
            pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid certificate pin"
        );
        ensure!(
            !capabilities.is_empty() && capabilities.len() <= 64,
            "select 1–64 capabilities"
        );
        let mut grants = Vec::new();
        for id in capabilities {
            let cap = self.capability(id)?;
            ensure!(cap.status == "active", "approve {id} before granting it");
            grants.push((id.clone(), cap.revision));
        }
        self.conn
            .execute("DELETE FROM tickets WHERE expires_at<?1", [now()])?;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expires_at = now() + 300;
        self.conn.execute(
            "INSERT INTO tickets(hash,expires_at,grants) VALUES(?1,?2,?3)",
            params![
                digest(token.as_bytes()),
                expires_at,
                serde_json::to_string(&grants)?
            ],
        )?;
        Ok(PairingTicket {
            version: 1,
            urls: Vec::new(),
            url,
            pin,
            token,
            expires_at,
        })
    }

    /// Consume a ticket atomically and enroll a key after proof of possession.
    ///
    /// # Errors
    /// Rejects invalid, expired, consumed, or stale tickets and invalid signatures.
    pub fn enroll(&mut self, request: Enrollment) -> Result<Device> {
        request.verify()?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let hash = digest(request.token.as_bytes());
        let body: String = tx
            .query_row(
                "SELECT grants FROM tickets WHERE hash=?1 AND expires_at>=?2",
                params![hash, now()],
                |r| r.get(0),
            )
            .context("invalid or expired ticket")?;
        let grants: Vec<(String, String)> = serde_json::from_str(&body)?;
        for (id, revision) in &grants {
            let valid: i64 = tx.query_row(
                "SELECT count(*) FROM capabilities WHERE id=?1 AND revision=?2 AND status='active'",
                params![id, revision],
                |r| r.get(0),
            )?;
            ensure!(valid == 1, "capability changed; request a new ticket");
        }
        let guarded: bool = tx.query_row(
            "SELECT enabled FROM enrollment_policy WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let device = Device {
            id: uuid::Uuid::new_v4().to_string(),
            name: request.name,
            alias: None,
            revoked: false,
            enrollment_status: if guarded { "pending" } else { "active" }.into(),
        };
        tx.execute(
            "INSERT INTO devices(id,name,public_key,enrollment_status) VALUES(?1,?2,?3,?4)",
            params![
                device.id,
                device.name,
                request.public_key,
                device.enrollment_status
            ],
        )?;
        if guarded {
            crate::enrollment::queue(&tx, &device, &request.public_key, &grants)?;
        } else {
            for (id, revision) in grants {
                tx.execute(
                    "INSERT INTO grants VALUES(?1,?2,?3)",
                    params![device.id, id, revision],
                )?;
            }
        }
        tx.execute("DELETE FROM tickets WHERE hash=?1", [hash])?;
        tx.commit()?;
        Ok(device)
    }

    /// Authenticate an envelope and persist its nonce before handling its payload.
    ///
    /// # Errors
    /// Rejects revoked devices, clock skew, forged requests and replays.
    pub fn authenticate(&self, request: &SignedRequest) -> Result<Request> {
        let key: Option<String> = self
            .conn
            .query_row(
                "SELECT public_key FROM devices WHERE id=?1 AND revoked=0",
                [&request.device_id],
                |r| r.get(0),
            )
            .optional()?;
        request.verify_for(
            CROSSLATCH_V1,
            &key.context("unknown or revoked device")?,
            now(),
        )?;
        let payload = serde_json::from_str(&request.payload)?;
        self.conn
            .execute("DELETE FROM nonces WHERE expires_at<?1", [now()])?;
        self.conn
            .execute(
                "INSERT INTO nonces VALUES(?1,?2,?3)",
                params![request.device_id, request.nonce, request.timestamp + 120],
            )
            .context("request replayed")?;
        Ok(payload)
    }

    /// List enrolled devices for the local operator.
    ///
    /// # Errors
    /// Returns `SQLite` errors.
    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,revoked,enrollment_status,alias FROM devices ORDER BY COALESCE(alias,name),id")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(Device {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    revoked: r.get(2)?,
                    enrollment_status: r.get(3)?,
                    alias: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Revoke a device and cancel its outstanding jobs.
    ///
    /// # Errors
    /// Returns unknown-device or `SQLite` errors.
    pub fn revoke(&mut self, id: &str) -> Result<()> {
        ensure!(
            !crate::enrollment::is_approver(&self.conn, id)?,
            "approver revocation requires a signed replacement; reset to a new server identity if the phone is lost"
        );
        let tx = self.conn.transaction()?;
        ensure!(
            tx.execute("UPDATE devices SET revoked=1 WHERE id=?1", [id])? == 1,
            "device not found"
        );
        tx.execute("INSERT INTO events(job_id,owner,status) SELECT id,owner,'cancelled' FROM jobs WHERE owner=?1 AND status IN ('queued','running')",[id])?;
        tx.execute(
            "UPDATE jobs SET status='cancelled' WHERE owner=?1 AND status IN ('queued','running')",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
}
