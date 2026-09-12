//! Security boundaries and restart behavior across public core operations.

#![expect(
    clippy::panic_in_result_fn,
    reason = "Test assertions fail the test while Result propagates fixture setup errors"
)]

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use xlatch_core::{
    auth::{Enrollment, SignedRequest},
    capability::{Execution, Manifest, Request},
    service,
    store::{Store, now},
    worker,
};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("xlatch-test-{}", uuid::Uuid::new_v4()));
        Store::initialize(&dir)?;
        Ok(Self(dir))
    }
    fn store(&self) -> Result<Store> {
        Store::open(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest() -> Manifest {
    Manifest {
        id: "echo".into(),
        title: "Echo".into(),
        description: "Return shared text".into(),
        accepts: vec!["text/plain".into()],
        input_schema: json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}},"additionalProperties":false}),
        output_schema: json!({"type":"object"}),
        execution: Execution::Echo,
        timeout_seconds: 5,
    }
}

fn pair(store: &mut Store, seed: u8) -> Result<(String, SigningKey)> {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let public_key = STANDARD.encode(key.verifying_key().as_bytes());
    let ticket = store.pairing_ticket(
        "https://localhost:7443".into(),
        "a".repeat(64),
        &["echo".into()],
    )?;
    let name = "test phone".to_string();
    let message = format!("xlatch.pair.v1\n{}\n{public_key}\n{name}", ticket.token);
    let enrollment = Enrollment {
        token: ticket.token,
        name,
        public_key,
        signature: STANDARD.encode(key.sign(message.as_bytes()).to_bytes()),
    };
    let device = store.enroll(enrollment.clone())?;
    assert!(store.enroll(enrollment).is_err(), "ticket is single use");
    Ok((device.id, key))
}

fn envelope(id: &str, key: &SigningKey, request: &Request) -> Result<SignedRequest> {
    let mut request = SignedRequest {
        device_id: id.into(),
        timestamp: now(),
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        payload: serde_json::to_string(&request)?,
        signature: String::new(),
    };
    request.signature = STANDARD.encode(key.sign(&request.signing_bytes()).to_bytes());
    Ok(request)
}

#[test]
fn approvals_are_revision_bound_and_input_is_typed() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    assert!(
        store
            .invoke(
                "local",
                "echo",
                &cap.revision,
                &json!({"text":"hello"}),
                "a"
            )
            .is_err()
    );
    store.approve("echo", &cap.revision, false)?;
    assert!(
        store
            .invoke("local", "echo", &cap.revision, &json!({"text":4}), "a")
            .is_err()
    );
    let job = store.invoke(
        "local",
        "echo",
        &cap.revision,
        &json!({"text":"hello"}),
        "a",
    )?;
    assert_eq!(
        job,
        store.invoke(
            "local",
            "echo",
            &cap.revision,
            &json!({"text":"hello"}),
            "a"
        )?
    );
    assert!(
        store
            .invoke(
                "local",
                "echo",
                &cap.revision,
                &json!({"text":"different"}),
                "a"
            )
            .is_err()
    );
    let mut changed = manifest();
    changed.description = "Changed action".into();
    let replacement = store.register(&changed)?;
    assert_eq!(replacement.status, "pending");
    assert!(store.approve("echo", &cap.revision, false).is_err());
    Ok(())
}

#[test]
fn key_proof_replay_clock_revocation_and_job_ownership() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (id, key) = pair(&mut store, 1)?;
    let (other, _) = pair(&mut store, 2)?;
    let request = envelope(&id, &key, &Request::Discover)?;
    store.authenticate(&request)?;
    assert!(store.authenticate(&request).is_err());
    let mut tampered = envelope(&id, &key, &Request::Discover)?;
    tampered.payload = "{\"op\":\"jobs\"}".into();
    assert!(store.authenticate(&tampered).is_err());
    let mut expired = envelope(&id, &key, &Request::Discover)?;
    expired.timestamp -= 1000;
    expired.signature = STANDARD.encode(key.sign(&expired.signing_bytes()).to_bytes());
    assert!(store.authenticate(&expired).is_err());
    let job = store.invoke(
        &id,
        "echo",
        &cap.revision,
        &json!({"text":"private"}),
        "device-job",
    )?;
    assert!(store.job(&other, &job.id).is_err());
    assert!(store.cancel(&other, &job.id).is_err());
    assert!(store.events(&other, 0)?.is_empty());
    store.revoke(&id)?;
    assert!(
        store
            .authenticate(&envelope(&id, &key, &Request::Jobs)?)
            .is_err()
    );
    assert_eq!(store.job("local", &job.id)?.status, "cancelled");
    assert!(
        store
            .invoke(&id, "echo", &cap.revision, &json!({"text":"no"}), "new")
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn persisted_job_runs_and_result_survives_reopen() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let job = store.invoke(
        "local",
        "echo",
        &cap.revision,
        &json!({"text":"round trip"}),
        "round-trip",
    )?;
    drop(store);
    let task = tokio::spawn(worker::run(fixture.0.clone()));
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let job = fixture.store()?.job("local", &job.id)?;
            if job.status == "succeeded" {
                return Ok::<_, anyhow::Error>(job);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .context("worker did not complete")??;
    task.abort();
    let _ = task.await;
    assert_eq!(result.result, Some(json!({"text":"round trip"})));
    let mut store = fixture.store()?;
    store.recover()?;
    assert_eq!(store.job("local", &job.id)?, result);
    let response = service::dispatch(&mut store, "local", Request::Events { after: 0 })?;
    assert_eq!(response.as_array().context("events array")?.len(), 3);
    Ok(())
}

#[test]
fn changed_capability_does_not_inherit_device_grants() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (id, _) = pair(&mut store, 3)?;
    let mut changed = manifest();
    changed.title = "Different echo".into();
    let new = store.register(&changed)?;
    store.approve("echo", &new.revision, false)?;
    assert!(store.discover(&id)?.is_empty());
    assert!(
        store
            .invoke(&id, "echo", &new.revision, &json!({"text":"no"}), "test")
            .is_err()
    );
    Ok(())
}

#[test]
fn restart_fails_interrupted_work_without_retrying_side_effects() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let job = store.invoke(
        "local",
        "echo",
        &cap.revision,
        &json!({"text":"interrupted"}),
        "restart",
    )?;
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute("UPDATE jobs SET status='running' WHERE id=?1", [&job.id])?;
    drop(conn);
    store.recover()?;
    let recovered = store.job("local", &job.id)?;
    assert_eq!(recovered.status, "failed");
    assert!(
        recovered
            .error
            .context("recovery diagnostic")?
            .contains("review side effects")
    );
    let events = store.events("local", 0)?;
    assert_eq!(events.last().context("recovery event")?.status, "failed");
    Ok(())
}

#[test]
fn schemas_do_not_fetch_remote_references() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut m = manifest();
    m.input_schema = json!({"$ref":"http://127.0.0.1/private-schema"});
    assert!(fixture.store()?.register(&m).is_err());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn host_execution_requires_consent_and_enforces_timeout_and_cancellation() -> Result<()> {
    use xlatch_core::capability::digest;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let program = "/usr/bin/python3";
    let mut m = manifest();
    m.id = "slow".into();
    m.timeout_seconds = 1;
    m.execution = Execution::Command {
        program: program.into(),
        args: vec!["-c".into(), "import time; time.sleep(30)".into()],
        sha256: digest(&std::fs::read(program)?),
    };
    let cap = store.register(&m)?;
    assert!(store.approve("slow", &cap.revision, false).is_err());
    store.approve("slow", &cap.revision, true)?;
    let job = store.invoke(
        "local",
        "slow",
        &cap.revision,
        &json!({"text":"timeout"}),
        "timeout",
    )?;
    let task = tokio::spawn(worker::run(fixture.0.clone()));
    let outcome = wait_for_status(&fixture, &job.id, "failed").await?;
    assert!(
        outcome
            .error
            .context("timeout diagnostic")?
            .contains("timed out")
    );
    let cancel = store.invoke(
        "local",
        "slow",
        &cap.revision,
        &json!({"text":"cancel"}),
        "cancel",
    )?;
    wait_for_status(&fixture, &cancel.id, "running").await?;
    store.cancel("local", &cancel.id)?;
    let cancelled = wait_for_status(&fixture, &cancel.id, "cancelled").await?;
    assert_eq!(cancelled.result, None);
    task.abort();
    let _ = task.await;
    Ok(())
}

#[cfg(unix)]
async fn wait_for_status(
    fixture: &Fixture,
    id: &str,
    status: &str,
) -> Result<xlatch_core::capability::Job> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let job = fixture.store()?.job("local", id)?;
            if job.status == status {
                return Ok::<_, anyhow::Error>(job);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .context("job did not reach expected status")?
}
