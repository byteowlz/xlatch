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

#[test]
fn newly_granted_actions_appear_without_pairing_again() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let initial = store.register(&manifest())?;
    store.approve("echo", &initial.revision, false)?;
    let (device, _) = pair(&mut store, 71)?;
    let mut added = manifest();
    added.id = "another-action".into();
    let pending = store.register(&added)?;
    assert!(store.grant(&device, &added.id, &pending.revision).is_err());
    store.approve(&added.id, &pending.revision, false)?;
    assert_eq!(store.discover(&device)?.len(), 1);
    assert!(store.grant(&device, &added.id, "stale").is_err());
    store.grant(&device, &added.id, &pending.revision)?;
    assert_eq!(
        store
            .discover(&device)?
            .iter()
            .map(|cap| cap.manifest.id.as_str())
            .collect::<Vec<_>>(),
        vec!["another-action", "echo"]
    );
    store.grant(&device, &added.id, &pending.revision)?;
    store.revoke(&device)?;
    assert!(store.grant(&device, &added.id, &pending.revision).is_err());
    Ok(())
}

fn enrollment_rpc(
    store: &mut Store,
    owner: &str,
    request: xlatch_core::enrollment::EnrollmentRequest,
) -> Result<serde_json::Value> {
    service::dispatch(store, owner, Request::Enrollment { request })
}

fn enable_guard(store: &mut Store, owner: &str) -> Result<p256::ecdsa::SigningKey> {
    use xlatch_core::enrollment::{EnrollmentRequest, enable_bytes};
    let key = p256::ecdsa::SigningKey::from_slice(&[42; 32])?;
    let public_key = STANDARD.encode(key.verifying_key().to_encoded_point(false).as_bytes());
    let bootstrap = store.enrollment_bootstrap(owner)?;
    let token = bootstrap["token"]
        .as_str()
        .context("bootstrap token")?
        .to_string();
    let status = enrollment_rpc(store, owner, EnrollmentRequest::Status)?;
    let server = status["server_id"].as_str().context("server identity")?;
    let signature: p256::ecdsa::Signature =
        key.sign(&enable_bytes(server, owner, &token, &public_key));
    let request = EnrollmentRequest::Enable {
        token,
        public_key,
        signature: STANDARD.encode(signature.to_der().as_bytes()),
    };
    let result = enrollment_rpc(store, owner, request.clone())?;
    assert_eq!(result["enabled"], true);
    assert!(
        enrollment_rpc(store, owner, request).is_err(),
        "bootstrap cannot be replayed"
    );
    assert!(
        store.enrollment_bootstrap(owner).is_err(),
        "CLI cannot replace an approver"
    );
    Ok(key)
}

fn decision(
    id: &str,
    payload: &str,
    key: &p256::ecdsa::SigningKey,
    approve: bool,
) -> xlatch_core::enrollment::EnrollmentRequest {
    let signature: p256::ecdsa::Signature =
        key.sign(&xlatch_core::enrollment::decision_bytes(payload, approve));
    xlatch_core::enrollment::EnrollmentRequest::Decide {
        id: id.into(),
        approve,
        signature: STANDARD.encode(signature.to_der().as_bytes()),
    }
}

#[test]
fn phone_approval_gates_enrollment_and_rejects_substitution_and_replay() -> Result<()> {
    use xlatch_core::enrollment::EnrollmentRequest;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (approver, _) = pair(&mut store, 30)?;
    let (legacy, _) = pair(&mut store, 31)?;
    let key = enable_guard(&mut store, &approver)?;
    assert!(
        store.revoke(&approver).is_err(),
        "CLI cannot silently remove the approver"
    );
    assert_eq!(
        store.discover(&legacy)?.len(),
        1,
        "existing grants survive opt-in"
    );
    assert!(enrollment_rpc(&mut store, &legacy, EnrollmentRequest::Pending).is_err());
    let (candidate, candidate_key) = pair(&mut store, 32)?;
    let rpc = envelope(&candidate, &candidate_key, &Request::Jobs)?;
    let request = store.authenticate(&rpc)?;
    assert!(service::dispatch(&mut store, &candidate, request).is_err());
    assert!(store.grant(&candidate, "echo", &cap.revision).is_err());
    assert!(store.discover(&candidate)?.is_empty());
    let state = enrollment_rpc(&mut store, &candidate, EnrollmentRequest::Status)?;
    assert_eq!(state["device_status"], "pending");
    let payload = state["pending_payload"]
        .as_str()
        .context("pending review")?;
    let mut review: serde_json::Value = serde_json::from_str(payload)?;
    for field in ["server_id", "public_key", "nonce", "device_id"] {
        let original = review[field].clone();
        review[field] = json!("substituted");
        let forged = decision(&candidate, &serde_json::to_string(&review)?, &key, true);
        assert!(
            enrollment_rpc(&mut store, &approver, forged).is_err(),
            "signature binds {field}"
        );
        review[field] = original;
    }
    let request = decision(&candidate, payload, &key, true);
    assert!(enrollment_rpc(&mut store, "local", request.clone()).is_err());
    assert!(enrollment_rpc(&mut store, &legacy, request.clone()).is_err());
    assert!(enrollment_rpc(&mut store, &candidate, request.clone()).is_err());
    let wrong_key = p256::ecdsa::SigningKey::from_slice(&[43; 32])?;
    assert!(
        enrollment_rpc(
            &mut store,
            &approver,
            decision(&candidate, payload, &wrong_key, true)
        )
        .is_err()
    );
    enrollment_rpc(&mut store, &approver, request.clone())?;
    assert!(enrollment_rpc(&mut store, &approver, request).is_err());
    assert_eq!(store.discover(&candidate)?.len(), 1);
    assert_eq!(
        enrollment_rpc(&mut store, &candidate, EnrollmentRequest::Status)?["is_approver"],
        false
    );
    drop(store);
    let mut reopened = Store::initialize(&fixture.0)?;
    assert_eq!(
        enrollment_rpc(&mut reopened, &approver, EnrollmentRequest::Status)?["enabled"],
        true
    );
    Ok(())
}

#[test]
fn enrollment_rechecks_expiry_revision_and_signed_decision() -> Result<()> {
    use xlatch_core::enrollment::EnrollmentRequest;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (approver, _) = pair(&mut store, 40)?;
    let key = enable_guard(&mut store, &approver)?;
    let (candidate, _) = pair(&mut store, 41)?;
    let status = enrollment_rpc(&mut store, &candidate, EnrollmentRequest::Status)?;
    let payload = status["pending_payload"].as_str().context("payload")?;
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute(
        "UPDATE pending_enrollments SET expires_at=0 WHERE device_id=?1",
        [&candidate],
    )?;
    assert!(
        enrollment_rpc(
            &mut store,
            &approver,
            decision(&candidate, payload, &key, true)
        )
        .is_err()
    );
    conn.execute(
        "UPDATE pending_enrollments SET expires_at=?2 WHERE device_id=?1",
        rusqlite::params![candidate, now() + 600],
    )?;
    let mut modified = manifest();
    modified.title = "Changed".into();
    store.register(&modified)?;
    assert!(
        enrollment_rpc(
            &mut store,
            &approver,
            decision(&candidate, payload, &key, true)
        )
        .is_err()
    );
    let mut flipped = decision(&candidate, payload, &key, false);
    if let EnrollmentRequest::Decide { approve, .. } = &mut flipped {
        *approve = true;
    }
    assert!(enrollment_rpc(&mut store, &approver, flipped).is_err());
    enrollment_rpc(
        &mut store,
        &approver,
        decision(&candidate, payload, &key, false),
    )?;
    assert_eq!(
        enrollment_rpc(&mut store, &candidate, EnrollmentRequest::Status)?["device_status"],
        "rejected"
    );
    Ok(())
}

#[test]
fn protected_control_rejects_operator_shortcuts_and_private_worker_access() {
    use xlatch_core::{
        executor::ExecutorRequest,
        local::{Control, authorize_control},
    };
    for request in [
        Control::Approve {
            id: "echo".into(),
            revision: "r".into(),
            allow_host_execution: true,
        },
        Control::Grant {
            device: "d".into(),
            id: "echo".into(),
            revision: "r".into(),
        },
        Control::Revoke { id: "d".into() },
        Control::EnrollmentBootstrap { device: "d".into() },
        Control::Rpc {
            request: Request::Jobs,
        },
    ] {
        assert!(authorize_control(&request, true).is_err());
    }
    let worker = Control::Executor {
        request: ExecutorRequest::Claim,
    };
    assert!(authorize_control(&worker, false).is_err());
    assert!(authorize_control(&worker, true).is_ok());
    assert!(authorize_control(&Control::Identity, true).is_ok());
}

#[test]
fn executor_leases_bind_results_and_do_not_replay_expired_work() -> Result<()> {
    use xlatch_core::executor::{self, ExecutorRequest, Work};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let job = store.invoke(
        "local",
        "echo",
        &cap.revision,
        &json!({"text":"test"}),
        "lease",
    )?;
    let work: Work =
        serde_json::from_value(executor::dispatch(&mut store, ExecutorRequest::Claim)?)?;
    assert_eq!(work.job.id, job.id);
    let complete = |lease: String, result| ExecutorRequest::Complete {
        id: job.id.clone(),
        lease,
        result: Some(result),
        error: None,
    };
    assert!(executor::dispatch(&mut store, complete("0".repeat(64), json!({}))).is_err());
    assert!(
        executor::dispatch(
            &mut store,
            complete(work.lease.clone(), json!("wrong schema"))
        )
        .is_err()
    );
    executor::dispatch(
        &mut store,
        complete(work.lease.clone(), json!({"text":"done"})),
    )?;
    assert!(executor::dispatch(&mut store, complete(work.lease, json!({}))).is_err());
    assert_eq!(
        store.job("local", &job.id)?.result,
        Some(json!({"text":"done"}))
    );
    let second = store.invoke(
        "local",
        "echo",
        &cap.revision,
        &json!({"text":"test"}),
        "expired",
    )?;
    let work: Work =
        serde_json::from_value(executor::dispatch(&mut store, ExecutorRequest::Claim)?)?;
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute("UPDATE executor_leases SET expires_at=0", [])?;
    store.expire_executor_leases()?;
    assert_eq!(store.job("local", &second.id)?.status, "failed");
    assert!(
        executor::dispatch(
            &mut store,
            ExecutorRequest::Complete {
                id: second.id,
                lease: work.lease,
                result: Some(json!({})),
                error: None
            }
        )
        .is_err()
    );
    assert!(executor::dispatch(&mut store, ExecutorRequest::Claim)?.is_null());
    Ok(())
}

#[test]
fn protected_migration_checks_phone_keys_and_revokes_unverified_devices() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (owner, rpc_key) = pair(&mut store, 70)?;
    let (other, _) = pair(&mut store, 71)?;
    let approval = enable_guard(&mut store, &owner)?;
    let anchor = xlatch_core::capability::digest(
        format!(
            "xlatch.protected.anchor.v1\n{}\n{owner}\n{}\n{}",
            store.server_identity()?,
            STANDARD.encode(rpc_key.verifying_key().as_bytes()),
            STANDARD.encode(approval.verifying_key().to_encoded_point(false).as_bytes())
        )
        .as_bytes(),
    );
    assert!(store.prepare_protected_migration(&"0".repeat(64)).is_err());
    assert_eq!(
        store.discover(&other)?.len(),
        1,
        "failed migration must not revoke clients"
    );
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute("INSERT INTO approvers VALUES(?1,'forged')", [&other])?;
    assert!(
        store.prepare_protected_migration(&anchor).is_err(),
        "reject injected additional approvers"
    );
    conn.execute("DELETE FROM approvers WHERE device_id=?1", [&other])?;
    store.prepare_protected_migration(&anchor)?;
    assert!(store.discover(&other)?.is_empty());
    assert_eq!(store.discover(&owner)?.len(), 1);
    Ok(())
}

#[test]
fn executor_protocol_preserves_successful_json_null() -> Result<()> {
    use xlatch_core::executor::ExecutorRequest;
    let value = json!({"action":"complete","id":"job","lease":"lease","result":null,"error":null});
    let request: ExecutorRequest = serde_json::from_value(value.clone())?;
    assert!(matches!(
        &request,
        ExecutorRequest::Complete {
            result: Some(serde_json::Value::Null),
            error: None,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(request)?, value);
    Ok(())
}

#[test]
fn registration_does_not_read_host_files() -> Result<()> {
    let fixture = Fixture::new()?;
    let store = fixture.store()?;
    let mut proposed = manifest();
    proposed.execution = Execution::Command {
        program: fixture
            .0
            .join("not-built-yet")
            .to_string_lossy()
            .into_owned(),
        args: vec![],
        sha256: "0".repeat(64),
    };
    let registered = store.register(&proposed)?;
    assert_eq!(registered.status, "pending");
    assert!(
        store
            .approve(&proposed.id, &registered.revision, true)
            .is_err(),
        "activation must still inspect the executable"
    );
    Ok(())
}
