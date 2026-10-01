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
        icon: None,
        file_input: None,
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
        Control::ParkedRead {
            id: "p".into(),
            directory: "/tmp".into(),
        },
        Control::ParkedDelete { id: "p".into() },
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
    let request = xlatch_core::approval::ApprovalRequest::Prepare {
        capability_id: "echo".into(),
        revision: cap.revision,
        devices: vec![],
    };
    let pending = prepared_payload(&mut store, &owner, request)?;
    store.prepare_protected_migration(&anchor)?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&pending, &approval, true)?
        )
        .is_err(),
        "migration invalidates outstanding approvals"
    );
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

fn approval_rpc(
    store: &mut Store,
    owner: &str,
    request: xlatch_core::approval::ApprovalRequest,
) -> Result<serde_json::Value> {
    service::dispatch(store, owner, Request::Approval { request })
}

fn capability_decision(
    payload: &str,
    key: &p256::ecdsa::SigningKey,
    approve: bool,
) -> Result<xlatch_core::approval::ApprovalRequest> {
    let review: xlatch_core::approval::ApprovalReview = serde_json::from_str(payload)?;
    let signature: p256::ecdsa::Signature =
        key.sign(&xlatch_core::approval::decision_bytes(payload, approve));
    Ok(xlatch_core::approval::ApprovalRequest::Decide {
        id: review.id,
        approve,
        signature: STANDARD.encode(signature.to_der().as_bytes()),
    })
}

#[test]
fn phone_capability_approval_binds_manifest_targets_and_decision() -> Result<()> {
    use xlatch_core::approval::ApprovalRequest;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (owner, request_key) = pair(&mut store, 70)?;
    let (other, _) = pair(&mut store, 71)?;
    let key = enable_guard(&mut store, &owner)?;
    let mut action = manifest();
    action.id = "new.action".into();
    let cap = store.register(&action)?;
    for caller in ["local", &other] {
        assert!(approval_rpc(&mut store, caller, ApprovalRequest::Catalog).is_err());
    }
    let prepare = ApprovalRequest::Prepare {
        capability_id: action.id.clone(),
        revision: cap.revision.clone(),
        devices: vec![other.clone()],
    };
    let payload = approval_rpc(&mut store, &owner, prepare)?
        .as_str()
        .context("review payload")?
        .to_owned();
    assert_eq!(store.capability(&action.id)?.status, "pending");
    for (pointer, value) in [
        ("/devices", json!([])),
        (
            "/manifest/execution",
            json!({"kind":"command","program":"/bin/sh","args":[],"sha256":"a".repeat(64)}),
        ),
        ("/server_id", json!("other-server")),
        ("/revision", json!("b".repeat(64))),
    ] {
        let mut altered: serde_json::Value = serde_json::from_str(&payload)?;
        *altered.pointer_mut(pointer).context("review field")? = value;
        let forged = capability_decision(&serde_json::to_string(&altered)?, &key, true)?;
        assert!(approval_rpc(&mut store, &owner, forged).is_err());
    }
    let mut swapped = capability_decision(&payload, &key, false)?;
    if let ApprovalRequest::Decide { approve, .. } = &mut swapped {
        *approve = true;
    }
    assert!(approval_rpc(&mut store, &owner, swapped).is_err());
    let signed = capability_decision(&payload, &key, true)?;
    assert!(approval_rpc(&mut store, &other, signed.clone()).is_err());
    assert!(approval_rpc(&mut store, "local", signed.clone()).is_err());
    let rpc = envelope(
        &owner,
        &request_key,
        &Request::Approval {
            request: signed.clone(),
        },
    )?;
    let authenticated = store.authenticate(&rpc)?;
    service::dispatch(&mut store, &owner, authenticated)?;
    assert!(approval_rpc(&mut store, &owner, signed).is_err());
    assert_eq!(store.capability(&action.id)?.status, "active");
    assert!(
        store
            .invoke(
                &owner,
                &action.id,
                &cap.revision,
                &json!({"text":"no grant"}),
                "owner"
            )
            .is_err()
    );
    store.invoke(
        &other,
        &action.id,
        &cap.revision,
        &json!({"text":"shared"}),
        "other",
    )?;
    let mut reopened = Store::initialize(&fixture.0)?;
    let payload = approval_rpc(
        &mut reopened,
        &owner,
        ApprovalRequest::Prepare {
            capability_id: action.id.clone(),
            revision: cap.revision.clone(),
            devices: vec![owner.clone()],
        },
    )?
    .as_str()
    .context("payload")?
    .to_owned();
    approval_rpc(
        &mut reopened,
        &owner,
        capability_decision(&payload, &key, true)?,
    )?;
    assert_eq!(
        reopened.discover(&other)?.len(),
        2,
        "additive grants retain previous access"
    );
    reopened.invoke(
        &owner,
        &action.id,
        &cap.revision,
        &json!({"text":"granted"}),
        "owner",
    )?;
    Ok(())
}

fn prepared_payload(
    store: &mut Store,
    owner: &str,
    request: xlatch_core::approval::ApprovalRequest,
) -> Result<String> {
    Ok(approval_rpc(store, owner, request)?
        .as_str()
        .context("review payload")?
        .to_owned())
}

#[test]
fn capability_approval_rechecks_races_expiry_and_rejection_atomically() -> Result<()> {
    use xlatch_core::approval::ApprovalRequest;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (owner, _) = pair(&mut store, 72)?;
    let (target, _) = pair(&mut store, 73)?;
    let key = enable_guard(&mut store, &owner)?;
    let mut action = manifest();
    action.id = "pending".into();
    let cap = store.register(&action)?;
    let prepare = ApprovalRequest::Prepare {
        capability_id: action.id.clone(),
        revision: cap.revision,
        devices: vec![owner.clone(), target.clone()],
    };
    let payload = prepared_payload(&mut store, &owner, prepare.clone())?;
    let replaced = prepared_payload(&mut store, &owner, prepare.clone())?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&payload, &key, true)?
        )
        .is_err()
    );
    let db = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    db.execute("UPDATE capability_approvals SET expires_at=0", [])?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&replaced, &key, true)?
        )
        .is_err()
    );
    let payload = prepared_payload(&mut store, &owner, prepare.clone())?;
    action.title = "Changed after review".into();
    store.register(&action)?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&payload, &key, true)?
        )
        .is_err()
    );
    action.title = "Echo".into();
    store.register(&action)?;
    let payload = prepared_payload(&mut store, &owner, prepare)?;
    store.revoke(&target)?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&payload, &key, true)?
        )
        .is_err()
    );
    assert_eq!(
        store.capability(&action.id)?.status,
        "pending",
        "activation rolls back when any target changes"
    );
    let grants: i64 = db.query_row(
        "SELECT count(*) FROM grants WHERE capability_id='pending'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(grants, 0, "earlier target writes roll back too");
    approval_rpc(
        &mut store,
        &owner,
        capability_decision(&payload, &key, false)?,
    )?;
    assert!(
        approval_rpc(
            &mut store,
            &owner,
            capability_decision(&payload, &key, true)?
        )
        .is_err()
    );
    assert_eq!(store.capability(&action.id)?.status, "pending");
    #[cfg(unix)]
    assert!(
        xlatch_core::local::authorize_control(
            &xlatch_core::local::Control::Rpc {
                request: Request::Approval {
                    request: ApprovalRequest::Catalog
                }
            },
            true
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn routing_history_is_opt_in_scoped_and_preserves_retry_identity() -> Result<()> {
    use xlatch_core::history::{Capture, HistoryPolicy};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve(&cap.manifest.id, &cap.revision, false)?;
    let input = json!({"text":"private content"});
    store.invoke(
        "local",
        &cap.manifest.id,
        &cap.revision,
        &input,
        "before-opt-in",
    )?;
    assert!(store.history(None)?.is_empty());
    let policy = HistoryPolicy {
        capture: Capture::Metadata,
        ..HistoryPolicy::default()
    };
    store.set_history_policy(&policy)?;
    let job = store.invoke("local", &cap.manifest.id, &cap.revision, &input, "captured")?;
    assert_eq!(
        store
            .invoke("local", &cap.manifest.id, &cap.revision, &input, "captured")?
            .id,
        job.id
    );
    let records = store.history(Some("local"))?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["job_id"], job.id);
    assert_eq!(records[0]["choice_provenance"], "unknown");
    assert!(!serde_json::to_string(&records)?.contains("private content"));
    assert!(store.history(Some("different-owner"))?.is_empty());
    assert_eq!(store.purge_history(Some("different-owner"))?, 0);
    assert_eq!(store.purge_history(Some("local"))?, 1);
    assert_eq!(
        store
            .invoke("local", &cap.manifest.id, &cap.revision, &input, "captured")?
            .id,
        job.id
    );
    assert!(store.history(None)?.is_empty());
    Ok(())
}

#[test]
fn routing_history_bounds_exclusions_and_restart() -> Result<()> {
    use xlatch_core::history::{Capture, HistoryPolicy};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve(&cap.manifest.id, &cap.revision, false)?;
    let mut policy = HistoryPolicy {
        capture: Capture::Content,
        max_rows: 1,
        ..HistoryPolicy::default()
    };
    store.set_history_policy(&policy)?;
    for key in ["one", "two"] {
        store.invoke(
            "local",
            &cap.manifest.id,
            &cap.revision,
            &json!({"text":key}),
            key,
        )?;
    }
    assert_eq!(fixture.store()?.history(None)?.len(), 1);
    assert_eq!(fixture.store()?.history_policy()?, policy);
    policy.excluded_capabilities.push(cap.manifest.id.clone());
    store.set_history_policy(&policy)?;
    store.purge_history(None)?;
    store.invoke(
        "local",
        &cap.manifest.id,
        &cap.revision,
        &json!({"text":"excluded"}),
        "excluded",
    )?;
    assert!(store.history(None)?.is_empty());
    policy.retention_days = 0;
    assert!(store.set_history_policy(&policy).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn protected_commands_reject_user_owned_entry_points() -> Result<()> {
    let fixture = Fixture::new()?;
    let program = fixture.0.join("mutable-script");
    std::fs::write(&program, "#!/usr/bin/env sh\necho unsafe\n")?;
    assert!(xlatch_core::host_trust::command(program.to_str().context("path")?).is_err());
    xlatch_core::host_trust::command("/bin/sh")?;
    Ok(())
}

fn recovery_rpc(
    store: &mut Store,
    owner: &str,
    request: xlatch_core::recovery::RecoveryRequest,
) -> Result<serde_json::Value> {
    enrollment_rpc(
        store,
        owner,
        xlatch_core::enrollment::EnrollmentRequest::Recovery { request },
    )
}
fn recovery_decision(
    payload: &str,
    key: &p256::ecdsa::SigningKey,
) -> Result<xlatch_core::recovery::RecoveryRequest> {
    let review: xlatch_core::recovery::Review = serde_json::from_str(payload)?;
    let sig: p256::ecdsa::Signature =
        key.sign(&xlatch_core::recovery::decision_bytes(payload, true));
    Ok(xlatch_core::recovery::RecoveryRequest::Decide {
        id: review.id,
        approve: true,
        signature: STANDARD.encode(sig.to_der().as_bytes()),
    })
}
#[test]
fn backup_approver_requires_independent_signature_and_can_revoke_lost_phone() -> Result<()> {
    use xlatch_core::recovery::{RecoveryRequest, proof_bytes};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (original, _) = pair(&mut store, 100)?;
    let (backup, _) = pair(&mut store, 101)?;
    let original_key = enable_guard(&mut store, &original)?;
    let backup_key = p256::ecdsa::SigningKey::from_slice(&[55; 32])?;
    let public_key = STANDARD.encode(
        backup_key
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes(),
    );
    let signature: p256::ecdsa::Signature = backup_key.sign(&proof_bytes(
        &store.server_identity()?,
        &backup,
        &public_key,
    ));
    let proposal = RecoveryRequest::Propose {
        public_key,
        signature: STANDARD.encode(signature.to_der().as_bytes()),
    };
    assert!(recovery_rpc(&mut store, "local", proposal.clone()).is_err());
    let raw = recovery_rpc(&mut store, &backup, proposal)?;
    let payload = raw.as_str().context("review")?;
    assert!(
        recovery_rpc(
            &mut store,
            &backup,
            recovery_decision(payload, &backup_key)?
        )
        .is_err()
    );
    let changed = payload.replace("add", "remove");
    assert!(
        recovery_rpc(
            &mut store,
            &original,
            recovery_decision(&changed, &original_key)?
        )
        .is_err()
    );
    let decision = recovery_decision(payload, &original_key)?;
    recovery_rpc(&mut store, &original, decision.clone())?;
    assert!(recovery_rpc(&mut store, &original, decision).is_err());
    assert!(store.revoke(&original).is_err());
    let raw = recovery_rpc(
        &mut store,
        &backup,
        RecoveryRequest::Remove {
            device_id: original.clone(),
        },
    )?;
    recovery_rpc(
        &mut store,
        &backup,
        recovery_decision(raw.as_str().context("remove review")?, &backup_key)?,
    )?;
    assert!(recovery_rpc(&mut store, &original, RecoveryRequest::Status).is_err());
    assert!(
        recovery_rpc(
            &mut store,
            &backup,
            RecoveryRequest::Remove {
                device_id: backup.clone()
            }
        )
        .is_err()
    );
    assert_eq!(
        recovery_rpc(&mut store, &backup, RecoveryRequest::Status)?["approvers"]
            .as_array()
            .context("approvers")?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn content_history_omits_binary_and_enforces_expiry_without_deleting_jobs() -> Result<()> {
    use xlatch_core::history::{Capture, HistoryPolicy};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let mut action = manifest();
    action.input_schema = json!({"type":"object"});
    let cap = store.register(&action)?;
    store.approve("echo", &cap.revision, false)?;
    store.set_history_policy(&HistoryPolicy {
        capture: Capture::Content,
        ..HistoryPolicy::default()
    })?;
    let input = json!({"text":"retained", "file":{"name":"note.txt","data_base64":"c2VjcmV0"}});
    let job = store.invoke("local", "echo", &cap.revision, &input, "binary")?;
    let records = store.history(None)?;
    assert_eq!(
        records[0]["input"],
        json!({"text":"retained","file":{"name":"note.txt"}})
    );
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute("UPDATE routing_history SET created_at=0", [])?;
    assert!(store.history(None)?.is_empty());
    assert_eq!(store.job("local", &job.id)?.input, input);
    Ok(())
}

fn composed_fixture(
    store: &mut Store,
    binding: xlatch_core::composition::Binding,
) -> Result<xlatch_core::capability::Capability> {
    use xlatch_core::composition::{Binding, Step};
    let mut first = manifest();
    first.output_schema = first.input_schema.clone();
    let cap = store.register(&first)?;
    store.approve("echo", &cap.revision, false)?;
    let mut second = first.clone();
    second.id = "second".into();
    let second_cap = store.register(&second)?;
    store.approve("second", &second_cap.revision, false)?;
    let mut composed = first.clone();
    composed.id = "composed".into();
    composed.timeout_seconds = 10;
    composed.execution = Execution::Compose {
        steps: vec![
            Step {
                manifest: first,
                revision: cap.revision,
                input: Binding::Previous,
            },
            Step {
                manifest: second,
                revision: second_cap.revision,
                input: binding,
            },
        ],
    };
    let result = store.register(&composed)?;
    store.approve("composed", &result.revision, false)?;
    Ok(result)
}
fn claim_step(store: &mut Store) -> Result<Option<xlatch_core::executor::Work>> {
    Ok(serde_json::from_value(xlatch_core::executor::dispatch(
        store,
        xlatch_core::executor::ExecutorRequest::Claim,
    )?)?)
}
fn complete_step(
    store: &mut Store,
    work: &xlatch_core::executor::Work,
    value: serde_json::Value,
) -> Result<()> {
    xlatch_core::executor::dispatch(
        store,
        xlatch_core::executor::ExecutorRequest::Complete {
            id: work.job.id.clone(),
            lease: work.lease.clone(),
            result: Some(value),
            error: None,
        },
    )?;
    Ok(())
}
#[test]
fn composition_delegates_only_reviewed_steps_and_checkpoints_across_restart() -> Result<()> {
    use xlatch_core::composition::Binding;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = composed_fixture(&mut store, Binding::Previous)?;
    let (owner, _) = pair(&mut store, 111)?;
    store.grant(&owner, "composed", &cap.revision)?;
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute(
        "DELETE FROM grants WHERE device_id=?1 AND capability_id='echo'",
        [&owner],
    )?;
    let input = json!({"text":"transcript"});
    let first_cap = store.capability("echo")?;
    assert!(
        store
            .invoke(&owner, "echo", &first_cap.revision, &input, "direct")
            .is_err()
    );
    let parent = store.invoke(&owner, "composed", &cap.revision, &input, "stable")?;
    let first = claim_step(&mut store)?.context("first step")?;
    assert_eq!(first.job.capability_id, "echo");
    complete_step(&mut store, &first, input.clone())?;
    drop(store);
    let mut store = fixture.store()?;
    store.recover()?;
    let second = claim_step(&mut store)?.context("second step")?;
    assert_eq!(second.job.capability_id, "second");
    assert_eq!(second.job.input, input);
    assert_ne!(second.job.id, first.job.id);
    complete_step(&mut store, &second, input.clone())?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job(&owner, &parent.id)?.status, "succeeded");
    assert_eq!(store.job(&owner, &parent.id)?.result, Some(input.clone()));
    assert_eq!(
        store
            .invoke(&owner, "composed", &cap.revision, &input, "stable")?
            .id,
        parent.id
    );
    assert_eq!(store.composition_steps(&owner, &parent.id)?.len(), 2);
    assert!(store.composition_steps("other", &parent.id).is_err());
    assert_eq!(store.jobs(&owner)?.len(), 1);
    assert!(
        store
            .events(&owner, 0)?
            .iter()
            .all(|event| event.job_id == parent.id)
    );
    Ok(())
}
#[test]
fn composition_mapping_failure_preserves_result_and_never_starts_next_step() -> Result<()> {
    use xlatch_core::composition::{Binding, Source};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let binding = Binding::Fields {
        fields: std::collections::BTreeMap::from([(
            "text".into(),
            Source::Previous {
                pointer: "/missing".into(),
            },
        )]),
    };
    let cap = composed_fixture(&mut store, binding)?;
    let parent = store.invoke(
        "local",
        "composed",
        &cap.revision,
        &json!({"text":"original"}),
        "map",
    )?;
    let first = claim_step(&mut store)?.context("step")?;
    complete_step(&mut store, &first, json!({"text":"saved transcript"}))?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job("local", &parent.id)?.status, "failed");
    assert_eq!(store.composition_steps("local", &parent.id)?.len(), 1);
    assert_eq!(
        store.job("local", &first.job.id)?.result,
        Some(json!({"text":"saved transcript"}))
    );
    Ok(())
}
#[test]
fn composition_cancellation_and_changed_dependencies_stop_delegation() -> Result<()> {
    use xlatch_core::composition::Binding;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = composed_fixture(&mut store, Binding::Previous)?;
    let input = json!({"text":"test"});
    let parent = store.invoke("local", "composed", &cap.revision, &input, "cancel")?;
    let first = claim_step(&mut store)?.context("step")?;
    store.cancel("local", &parent.id)?;
    complete_step(&mut store, &first, input.clone())?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job("local", &first.job.id)?.status, "cancelled");
    let parent = store.invoke("local", "composed", &cap.revision, &input, "change")?;
    let first = claim_step(&mut store)?.context("step")?;
    complete_step(&mut store, &first, input)?;
    let mut changed = store.capability("second")?.manifest;
    changed.title = "changed".into();
    store.register(&changed)?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job("local", &parent.id)?.status, "failed");
    assert_eq!(store.composition_steps("local", &parent.id)?.len(), 1);
    Ok(())
}
#[test]
fn composition_types_are_conservative_and_mappings_do_not_coerce() -> Result<()> {
    use xlatch_core::composition::{Binding, Source, compatible};
    let text = json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}},"additionalProperties":false});
    assert!(!compatible(&json!({"type":"object"}), &text));
    assert!(compatible(&text, &text));
    assert!(!compatible(
        &text,
        &json!({"type":"object","additionalProperties":{"type":"integer"}})
    ));
    let binding = Binding::Fields {
        fields: std::collections::BTreeMap::from([
            (
                "text".into(),
                Source::Previous {
                    pointer: "/transcript".into(),
                },
            ),
            (
                "mime_type".into(),
                Source::Literal {
                    value: json!("text/plain"),
                },
            ),
        ]),
    };
    assert_eq!(
        binding.apply(&json!({}), &json!({"transcript":"hello"}))?,
        json!({"text":"hello","mime_type":"text/plain"})
    );
    assert!(binding.apply(&json!({}), &json!({})).is_err());
    Ok(())
}

#[test]
fn composition_enforces_final_receipt_and_never_replays_uncertain_step() -> Result<()> {
    use xlatch_core::composition::Binding;
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let mut cap = composed_fixture(&mut store, Binding::Previous)?;
    cap.manifest.output_schema =
        json!({"type":"object","required":["ok"],"properties":{"ok":{"const":true}}});
    cap = store.register(&cap.manifest)?;
    store.approve("composed", &cap.revision, false)?;
    let input = json!({"text":"test"});
    let parent = store.invoke("local", "composed", &cap.revision, &input, "receipt")?;
    let first = claim_step(&mut store)?.context("first")?;
    complete_step(&mut store, &first, input.clone())?;
    let second = claim_step(&mut store)?.context("second")?;
    complete_step(&mut store, &second, input.clone())?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job("local", &parent.id)?.status, "failed");
    let parent = store.invoke("local", "composed", &cap.revision, &input, "interrupted")?;
    let running = claim_step(&mut store)?.context("running")?;
    store.recover()?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job("local", &parent.id)?.status, "failed");
    assert_eq!(store.composition_steps("local", &parent.id)?.len(), 1);
    assert_eq!(store.job("local", &running.job.id)?.status, "failed");
    Ok(())
}

#[test]
fn one_off_chain_requires_each_grant_and_never_activates_targets() -> Result<()> {
    use xlatch_core::{chain::Reference, composition::Binding};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    composed_fixture(&mut store, Binding::Previous)?;
    let (owner, _) = pair(&mut store, 115)?;
    let second = store.capability("second")?;
    let refs = vec![
        Reference {
            capability_id: "echo".into(),
            revision: store.capability("echo")?.revision,
        },
        Reference {
            capability_id: "second".into(),
            revision: second.revision.clone(),
        },
    ];
    let input = json!({"text":"chain"});
    assert!(store.invoke_chain(&owner, &refs, &input, "chain").is_err());
    store.grant(&owner, "second", &second.revision)?;
    let before = store.discover("local")?;
    assert!(
        store
            .chain_candidates(&owner, &refs[..1])?
            .iter()
            .any(|c| c.manifest.id == "second")
    );
    let parent = store.invoke_chain(&owner, &refs, &input, "chain")?;
    assert_eq!(
        store.invoke_chain(&owner, &refs, &input, "chain")?.id,
        parent.id
    );
    assert!(
        store
            .invoke_chain(&owner, &refs, &json!({"text":"changed"}), "chain")
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(store.discover("local")?)?,
        serde_json::to_value(before)?
    );
    let first = claim_step(&mut store)?.context("first")?;
    complete_step(&mut store, &first, input.clone())?;
    drop(store);
    let mut store = fixture.store()?;
    store.recover()?;
    let second_job = claim_step(&mut store)?.context("second")?;
    complete_step(&mut store, &second_job, input.clone())?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job(&owner, &parent.id)?.result, Some(input));
    let saved = store.save_chain(&owner, &refs, "Saved chain")?;
    assert_eq!(saved.status, "pending");
    assert!(
        store
            .invoke(
                &owner,
                &saved.manifest.id,
                &saved.revision,
                &json!({"text":"x"}),
                "saved"
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn one_off_chain_stops_after_grant_revocation() -> Result<()> {
    use xlatch_core::{chain::Reference, composition::Binding};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    composed_fixture(&mut store, Binding::Previous)?;
    let (owner, _) = pair(&mut store, 116)?;
    let second = store.capability("second")?;
    store.grant(&owner, "second", &second.revision)?;
    let refs = vec![
        Reference {
            capability_id: "echo".into(),
            revision: store.capability("echo")?.revision,
        },
        Reference {
            capability_id: "second".into(),
            revision: second.revision,
        },
    ];
    let parent = store.invoke_chain(&owner, &refs, &json!({"text":"x"}), "revoked")?;
    let first = claim_step(&mut store)?.context("first")?;
    complete_step(&mut store, &first, json!({"text":"x"}))?;
    let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    conn.execute(
        "DELETE FROM grants WHERE device_id=?1 AND capability_id='second'",
        [&owner],
    )?;
    assert!(claim_step(&mut store)?.is_none());
    assert_eq!(store.job(&owner, &parent.id)?.status, "failed");
    assert_eq!(store.composition_steps(&owner, &parent.id)?.len(), 1);
    Ok(())
}

#[test]
fn icons_preserve_legacy_revisions_and_reject_active_svg_content() -> Result<()> {
    use xlatch_core::icon::Icon;
    let legacy = r#"{"id":"echo","title":"Echo","description":"Return shared text","accepts":["text/plain"],"input_schema":{"additionalProperties":false,"properties":{"text":{"type":"string"}},"required":["text"],"type":"object"},"output_schema":{"type":"object"},"execution":{"kind":"echo"},"timeout_seconds":5}"#;
    let mut action: Manifest = serde_json::from_str(legacy)?;
    assert_eq!(serde_json::to_string(&action)?, legacy);
    let fixture = Fixture::new()?;
    let path = fixture.0.join("icon.svg");
    std::fs::write(
        &path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#008877" d="M2 2h20v20H2z"/></svg>"##,
    )?;
    let dark_path = fixture.0.join("icon-dark.svg");
    std::fs::write(
        &dark_path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#ffffff" d="M2 2h20v20H2z"/></svg>"##,
    )?;
    let icon = Icon::from_file(&path)?.with_dark_file(&dark_path)?;
    let png = icon.png_bytes()?;
    assert!(png.starts_with(b"\x89PNG"));
    assert_eq!(
        image::load_from_memory(&png)?
            .to_rgba8()
            .get_pixel(64, 64)
            .0,
        [0, 136, 119, 255]
    );
    assert_eq!(
        image::load_from_memory(&icon.png_bytes_for(true)?)?
            .to_rgba8()
            .get_pixel(64, 64)
            .0,
        [255, 255, 255, 255]
    );
    let mut too_large = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(257, 1).write_to(&mut too_large, image::ImageFormat::Png)?;
    assert!(
        Icon {
            png_base64: STANDARD.encode(too_large.into_inner()),
            dark_png_base64: None,
        }
        .png_bytes()
        .is_err()
    );
    action.icon = Some(icon);
    action.validate()?;
    assert_ne!(action.revision()?, manifest().revision()?);
    for unsafe_svg in [
        r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///etc/passwd"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect onclick="alert(1)"/></svg>"#,
        r##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#loop" id="loop"/></svg>"##,
    ] {
        std::fs::write(&path, unsafe_svg)?;
        assert!(Icon::from_file(&path).is_err());
    }
    assert!(
        Icon {
            png_base64: "not png".into(),
            dark_png_base64: None,
        }
        .png_bytes()
        .is_err()
    );
    assert!(
        Icon {
            png_base64: "A".repeat(175_001),
            dark_png_base64: None,
        }
        .png_bytes()
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn large_upload_resumes_and_saves_without_changing_legacy_manifest() -> Result<()> {
    use xlatch_core::uploads::{CHUNK_BYTES, UploadRequest};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let destination = fixture.0.join("incoming");
    std::fs::create_dir(&destination)?;
    let destination = std::fs::canonicalize(destination)?;
    let mut action = manifest();
    action.execution = Execution::SaveFile {
        directory: destination.to_string_lossy().into(),
    };
    let legacy: Manifest =
        serde_json::from_str(include_str!("../../../examples/capabilities/echo.json"))?;
    action.input_schema = legacy.input_schema;
    let cap = store.register(&action)?;
    store.approve("echo", &cap.revision, true)?;
    let id = uuid::Uuid::new_v4().to_string();
    let bytes = vec![37u8; CHUNK_BYTES * 9 + 31];
    let begin = || UploadRequest::Begin {
        id: id.clone(),
        name: "recording.wav".into(),
        mime_type: "audio/wav".into(),
        size: bytes.len() as u64,
    };
    xlatch_core::uploads::dispatch(&store, "local", begin())?;
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let request = || UploadRequest::Chunk {
            id: id.clone(),
            offset: (index * CHUNK_BYTES) as u64,
            data_base64: STANDARD.encode(chunk),
        };
        let result = xlatch_core::uploads::dispatch(&store, "local", request())?;
        assert_eq!(
            xlatch_core::uploads::dispatch(&store, "local", request())?,
            result
        );
        drop(store);
        store = fixture.store()?;
        assert_eq!(
            xlatch_core::uploads::dispatch(&store, "local", begin())?["offset"],
            result["offset"]
        );
    }
    let input = json!({"mime_type":"audio/wav","file":{"artifact_id":id,"name":"recording.wav","mime_type":"audio/wav","size":bytes.len()}});
    let mut constrained = action.clone();
    constrained.id = "limited-save".into();
    constrained.input_schema["properties"]["file"]["properties"]["data_base64"]["maxLength"] =
        json!(4);
    let limited = store.register(&constrained)?;
    store.approve("limited-save", &limited.revision, true)?;
    assert!(
        store
            .invoke(
                "local",
                "limited-save",
                &limited.revision,
                &input,
                "cannot-bypass-size"
            )
            .is_err()
    );
    let job = store.invoke("local", "echo", &cap.revision, &input, "large-file")?;
    assert_eq!(
        store
            .invoke("local", "echo", &cap.revision, &input, "large-file")?
            .id,
        job.id
    );
    assert!(xlatch_core::uploads::dispatch(&store, "local", UploadRequest::Abort { id }).is_err());
    drop(store);
    let task = tokio::spawn(worker::run(fixture.0.clone()));
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job = fixture.store()?.job("local", &job.id)?;
            if !matches!(job.status.as_str(), "queued" | "running") {
                return Ok::<_, anyhow::Error>(job);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await?;
    task.abort();
    let _ = task.await;
    let result = result?;
    assert_eq!(result.status, "succeeded", "{:?}", result.error);
    assert_eq!(std::fs::read(destination.join("recording.wav"))?, bytes);
    assert_eq!(
        fixture.store()?.discover("local")?[0].revision,
        cap.revision
    );
    assert!(
        !std::fs::read_dir(&fixture.0)?
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(".xlatch-file-"))
    );
    Ok(())
}

#[test]
fn uploads_reject_other_owners_changed_chunks_and_storage_overcommit() -> Result<()> {
    use xlatch_core::uploads::{CHUNK_BYTES, UploadPolicy, UploadRequest, dispatch};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (phone, _) = pair(&mut store, 8)?;
    let (other, _) = pair(&mut store, 9)?;
    store.set_upload_policy(&UploadPolicy {
        max_file_bytes: None,
        quota_bytes: CHUNK_BYTES as u64 * 2,
        retention_hours: 1,
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    let begin = || UploadRequest::Begin {
        id: id.clone(),
        name: "file.bin".into(),
        mime_type: "application/octet-stream".into(),
        size: CHUNK_BYTES as u64 * 2,
    };
    dispatch(&store, &phone, begin())?;
    assert!(dispatch(&store, &other, begin()).is_err());
    assert!(
        dispatch(
            &store,
            &phone,
            UploadRequest::Begin {
                id: uuid::Uuid::new_v4().to_string(),
                name: "other.bin".into(),
                mime_type: "application/octet-stream".into(),
                size: 1
            }
        )
        .is_err()
    );
    let chunk = |offset, byte| UploadRequest::Chunk {
        id: id.clone(),
        offset,
        data_base64: STANDARD.encode(vec![byte; CHUNK_BYTES]),
    };
    assert!(dispatch(&store, &phone, chunk(CHUNK_BYTES as u64, 42)).is_err());
    assert!(dispatch(&store, &other, chunk(0, 42)).is_err());
    dispatch(&store, &phone, chunk(0, 42))?;
    assert!(dispatch(&store, &phone, chunk(0, 43)).is_err());
    dispatch(&store, &phone, chunk(CHUNK_BYTES as u64, 43))?;
    let mut action = manifest();
    action.file_input = Some(xlatch_core::uploads::FileInput::Path);
    action.input_schema = json!({"type":"object"});
    let cap = store.register(&action)?;
    store.approve("echo", &cap.revision, false)?;
    store.grant(&phone, "echo", &cap.revision)?;
    store.grant(&other, "echo", &cap.revision)?;
    let input = json!({"file":{"artifact_id":id,"name":"file.bin","mime_type":"application/octet-stream","size":CHUNK_BYTES * 2}});
    assert!(
        store
            .invoke(&other, "echo", &cap.revision, &input, "stolen")
            .is_err()
    );
    assert!(
        store
            .invoke(
                &phone,
                "echo",
                &cap.revision,
                &json!({"file":{"path":"/etc/passwd"}}),
                "injected"
            )
            .is_err()
    );
    store.revoke(&phone)?;
    assert!(service::dispatch(&mut store, &phone, Request::Upload { request: begin() }).is_err());
    Ok(())
}

#[test]
fn protected_upload_reads_require_live_job_lease_and_preserve_active_bytes() -> Result<()> {
    use xlatch_core::{
        executor::{self, ExecutorRequest, Work},
        uploads::{self, FileInput, UploadRequest},
    };
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let mut action = manifest();
    action.file_input = Some(FileInput::Path);
    action.input_schema = json!({"type":"object"});
    let cap = store.register(&action)?;
    store.approve("echo", &cap.revision, false)?;
    let (phone, _) = pair(&mut store, 11)?;
    let id = uuid::Uuid::new_v4().to_string();
    uploads::dispatch(
        &store,
        &phone,
        UploadRequest::Begin {
            id: id.clone(),
            name: "a.bin".into(),
            mime_type: "application/octet-stream".into(),
            size: 3,
        },
    )?;
    uploads::dispatch(
        &store,
        &phone,
        UploadRequest::Chunk {
            id: id.clone(),
            offset: 0,
            data_base64: STANDARD.encode(b"abc"),
        },
    )?;
    let job = store.invoke(&phone,"echo",&cap.revision,&json!({"file":{"artifact_id":id,"name":"a.bin","mime_type":"application/octet-stream","size":3}}),"upload-lease")?;
    let work: Work =
        serde_json::from_value(executor::dispatch(&mut store, ExecutorRequest::Claim)?)?;
    let read = |lease: String| ExecutorRequest::ReadUpload {
        id: job.id.clone(),
        lease,
        offset: 0,
    };
    assert!(executor::dispatch(&mut store, read("0".repeat(64))).is_err());
    assert_eq!(
        executor::dispatch(&mut store, read(work.lease.clone()))?,
        json!({"data_base64":STANDARD.encode(b"abc")})
    );
    let db = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
    db.execute("UPDATE uploads SET expires=0", [])?;
    store.prune_uploads()?;
    assert_eq!(
        db.query_row("SELECT count(*) FROM upload_chunks", [], |r| r
            .get::<_, i64>(0))?,
        1
    );
    store.revoke(&phone)?;
    assert!(executor::dispatch(&mut store, read(work.lease)).is_err());
    store.prune_uploads()?;
    assert_eq!(
        db.query_row("SELECT count(*) FROM upload_chunks", [], |r| r
            .get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn device_aliases_and_signed_removal_preserve_identity_and_cancel_jobs() -> Result<()> {
    use xlatch_core::device_management::{Change, DeviceRequest, decision_bytes, dispatch};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (phone, _) = pair(&mut store, 110)?;
    let (target, _) = pair(&mut store, 111)?;
    let key = enable_guard(&mut store, &phone)?;
    let job = store.invoke(
        &target,
        "echo",
        &cap.revision,
        &json!({"text":"queued"}),
        "remove-device",
    )?;
    let original = store
        .devices()?
        .into_iter()
        .find(|d| d.id == target)
        .context("device")?
        .name;
    store.change_device_local(
        &target,
        Change::Alias {
            alias: Some("MacBook".into()),
        },
        false,
    )?;
    let device = store
        .devices()?
        .into_iter()
        .find(|d| d.id == target)
        .context("device")?;
    assert_eq!(
        (device.name, device.alias),
        (original, Some("MacBook".into()))
    );
    assert!(dispatch(&mut store, &target, DeviceRequest::List).is_err());
    assert!(
        store
            .change_device_local(
                &target,
                Change::Alias {
                    alias: Some("\n".into())
                },
                false
            )
            .is_err()
    );
    let queued = store.change_device_local(&target, Change::Remove, true)?;
    assert_eq!(queued["pending"], true);
    assert!(
        !store
            .devices()?
            .into_iter()
            .find(|d| d.id == target)
            .context("device")?
            .revoked
    );
    let payload = queued["payload"].as_str().context("review")?;
    let review: serde_json::Value = serde_json::from_str(payload)?;
    let id = review["id"].as_str().context("review id")?.to_owned();
    let signature: p256::ecdsa::Signature = key.sign(&decision_bytes(payload, true));
    let request = DeviceRequest::Decide {
        id,
        approve: true,
        signature: STANDARD.encode(signature.to_der().as_bytes()),
    };
    let mut forged = request.clone();
    if let DeviceRequest::Decide { approve, .. } = &mut forged {
        *approve = false;
    }
    assert!(dispatch(&mut store, &phone, forged).is_err());
    dispatch(&mut store, &phone, request.clone())?;
    assert!(dispatch(&mut store, &phone, request).is_err());
    assert!(
        store
            .devices()?
            .into_iter()
            .find(|d| d.id == target)
            .context("device")?
            .revoked
    );
    assert_eq!(store.job(&target, &job.id)?.status, "cancelled");
    assert!(
        store
            .change_device_local(&phone, Change::Remove, false)
            .is_err()
    );
    let payload = dispatch(
        &mut store,
        &phone,
        DeviceRequest::Prepare {
            id: phone.clone(),
            change: Change::Remove,
        },
    )?;
    let payload = payload.as_str().context("self review")?;
    let review: serde_json::Value = serde_json::from_str(payload)?;
    let signature: p256::ecdsa::Signature = key.sign(&decision_bytes(payload, true));
    assert!(
        dispatch(
            &mut store,
            &phone,
            DeviceRequest::Decide {
                id: review["id"].as_str().context("id")?.into(),
                approve: true,
                signature: STANDARD.encode(signature.to_der().as_bytes())
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn changed_or_expired_device_reviews_cannot_be_applied() -> Result<()> {
    use xlatch_core::device_management::{Change, DeviceRequest, decision_bytes, dispatch};
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let cap = store.register(&manifest())?;
    store.approve("echo", &cap.revision, false)?;
    let (phone, _) = pair(&mut store, 112)?;
    let key = enable_guard(&mut store, &phone)?;
    for expired in [false, true] {
        let queued = store.change_device_local(
            &phone,
            Change::Alias {
                alias: Some("Reviewed".into()),
            },
            true,
        )?;
        let payload = queued["payload"].as_str().context("payload")?;
        let review: serde_json::Value = serde_json::from_str(payload)?;
        let id = review["id"].as_str().context("id")?;
        if expired {
            let conn = rusqlite::Connection::open(fixture.0.join("xlatch.sqlite3"))?;
            conn.execute("UPDATE device_reviews SET expires_at=0 WHERE id=?1", [id])?;
        } else {
            store.change_device_local(
                &phone,
                Change::Alias {
                    alias: Some("Newer name".into()),
                },
                false,
            )?;
        }
        let sig: p256::ecdsa::Signature = key.sign(&decision_bytes(payload, true));
        assert!(
            dispatch(
                &mut store,
                &phone,
                DeviceRequest::Decide {
                    id: id.into(),
                    approve: true,
                    signature: STANDARD.encode(sig.to_der().as_bytes())
                }
            )
            .is_err()
        );
    }
    store.change_device_local(&phone, Change::Alias { alias: None }, false)?;
    assert_eq!(
        store
            .devices()?
            .into_iter()
            .find(|d| d.id == phone)
            .context("phone")?
            .alias,
        None
    );
    Ok(())
}

#[test]
fn parked_content_is_owner_scoped_and_dispatches_once() -> Result<()> {
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let mut parked_manifest = manifest();
    parked_manifest.input_schema = json!({"type":"object","required":["text","mime_type"],"properties":{"text":{"type":"string"},"mime_type":{"const":"text/plain"}},"additionalProperties":false});
    let capability = store.register(&parked_manifest)?;
    store.approve("echo", &capability.revision, false)?;
    let (phone, _) = pair(&mut store, 121)?;
    let (other, _) = pair(&mut store, 122)?;
    let id = uuid::Uuid::new_v4().to_string();
    let input = json!({"text":"read this later","mime_type":"text/plain"});

    let item = store.park(&phone, &id, "read this later", "text/plain", &input)?;
    assert_eq!(store.parked(&phone)?, vec![item.clone()]);
    assert!(store.parked(&other)?.is_empty());
    assert!(store.delete_parked(&other, &id).is_err());
    assert_eq!(
        store.park(&phone, &id, "read this later", "text/plain", &input)?,
        item
    );
    assert_eq!(store.parked_candidates(&phone, &id)?[0].manifest.id, "echo");
    let export = fixture.0.join("later");
    std::fs::create_dir(&export)?;
    let export = std::fs::canonicalize(export)?;
    assert_eq!(store.read_parked(&phone, &id, &export)?.input, input);
    assert_eq!(store.parked(&phone)?.len(), 1, "reading must not consume");

    let job = store.dispatch_parked(&phone, &id, "echo", &capability.revision)?;
    assert!(store.parked(&phone)?.is_empty());
    assert_eq!(job.input, input);
    assert_eq!(
        store.dispatch_parked(&phone, &id, "echo", &capability.revision)?,
        job
    );
    Ok(())
}

#[test]
fn parked_preparation_caches_typed_result_without_risking_original() -> Result<()> {
    use xlatch_core::{
        chain::Reference,
        executor::{self, ExecutorRequest, Work},
    };
    let fixture = Fixture::new()?;
    let mut store = fixture.store()?;
    let mut prepare = manifest();
    prepare.accepts = vec!["text/uri-list".into()];
    prepare.input_schema = json!({
        "type":"object",
        "required":["text","mime_type"],
        "properties":{"text":{"type":"string"},"mime_type":{"const":"text/uri-list"}},
        "additionalProperties":false
    });
    let capability = store.register(&prepare)?;
    store.approve("echo", &capability.revision, false)?;
    let (phone, _) = pair(&mut store, 123)?;
    let id = uuid::Uuid::new_v4().to_string();
    let input = json!({"text":"https://example.com","mime_type":"text/uri-list"});
    let reference = Reference {
        capability_id: "echo".into(),
        revision: capability.revision,
    };

    let item = store.park_with_preparation(
        &phone,
        &id,
        "Example",
        "text/uri-list",
        &input,
        Some(&reference),
    )?;
    assert_eq!(
        item.preparation.as_ref().context("preparation")?.status,
        "queued"
    );
    let export = std::fs::canonicalize(&fixture.0)?;
    let pending = store.read_parked(&phone, &id, &export)?;
    assert_eq!(pending.input, input);
    assert_eq!(pending.prepared, None);

    let work: Work =
        serde_json::from_value(executor::dispatch(&mut store, ExecutorRequest::Claim)?)?;
    executor::dispatch(
        &mut store,
        ExecutorRequest::Complete {
            id: work.job.id,
            lease: work.lease,
            result: Some(json!({"text":"cached article"})),
            error: None,
        },
    )?;
    let ready = store.read_parked(&phone, &id, &export)?;
    assert_eq!(ready.input, input);
    assert_eq!(ready.prepared, Some(json!({"text":"cached article"})));
    assert_eq!(
        ready.item.preparation.context("preparation")?.status,
        "succeeded"
    );
    assert_eq!(
        store.parked(&phone)?.len(),
        1,
        "preparation must not consume"
    );
    Ok(())
}

#[test]
fn parked_file_read_materializes_safe_bytes_without_consuming() -> Result<()> {
    let fixture = Fixture::new()?;
    let store = fixture.store()?;
    let id = uuid::Uuid::new_v4().to_string();
    let input = json!({
        "mime_type":"application/octet-stream",
        "file":{
            "name":"../../notes.bin",
            "mime_type":"application/octet-stream",
            "data_base64":STANDARD.encode(b"parked bytes")
        }
    });
    store.park(
        "local",
        &id,
        "notes.bin",
        "application/octet-stream",
        &input,
    )?;
    let export = fixture.0.join("later-file");
    std::fs::create_dir(&export)?;
    let export = std::fs::canonicalize(export)?;
    let content = store.read_parked("local", &id, &export)?;
    let path = content.input["file"]["path"]
        .as_str()
        .context("materialized path")?;
    assert_eq!(std::fs::read(path)?, b"parked bytes");
    assert_eq!(content.input["file"]["name"], "notes.bin");
    assert_eq!(store.parked("local")?.len(), 1);
    store.delete_parked("local", &id)?;
    assert!(store.parked("local")?.is_empty());
    Ok(())
}
