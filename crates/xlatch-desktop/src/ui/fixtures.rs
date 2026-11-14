//! Synthetic review data only. This path never calls a transport or user capture source.
use super::{
    Capability, Connection, Control, Desktop, Draft, Job, Page, Request, Snapshot, Value, json,
};
use xlatch_core::capability::{Execution, Manifest};

pub(super) fn capability() -> Capability {
    Capability {
        manifest: Manifest {
            id: "echo".into(),
            title: "Echo text".into(),
            description: "Return explicitly supplied text without side effects.".into(),
            icon: None,
            file_input: None,
            accepts: vec!["text/plain".into()],
            input_schema: json!({"type":"object","required":["text","mime_type"],"properties":{"text":{"type":"string"},"mime_type":{"type":"string"}},"additionalProperties":false}),
            output_schema: json!(true),
            execution: Execution::Echo,
            timeout_seconds: 10,
        },
        revision: "a".repeat(64),
        status: "active".into(),
    }
}

fn snapshot() -> Snapshot {
    let action = capability();
    let jobs = ["queued", "running", "succeeded", "failed", "cancelled"]
        .into_iter()
        .enumerate()
        .map(|(i, status)| Job {
            id: format!("synthetic-job-{i}"),
            capability_id: action.manifest.id.clone(),
            revision: action.revision.clone(),
            owner: "synthetic-review".into(),
            status: status.into(),
            input: json!({"text":"Synthetic review only","mime_type":"text/plain"}),
            result: (status == "succeeded")
                .then(|| json!({"text":"Synthetic review only","mime_type":"text/plain"})),
            error: (status == "failed").then(|| "Synthetic failure".into()),
            created_at: 0,
        })
        .collect();
    Snapshot {
        capabilities: vec![action],
        jobs,
    }
}

pub(super) fn actions(s: &mut Desktop) {
    s.fixture = true;
    s.refresh_pending = false;
    s.busy = false;
    s.page = Page::Actions;
    s.snapshot = Some(snapshot());
    s.selected = None;
    s.draft = None;
    s.result = None;
    s.error = None;
    s.appearance.status = "Bundled fixture palette · no user configuration".into();
}
pub(super) fn activity(s: &mut Desktop) {
    actions(s);
    s.page = Page::Activity;
}
pub(super) fn connection(s: &mut Desktop) {
    actions(s);
    s.page = Page::Connection;
}
pub(super) fn settings(s: &mut Desktop) {
    actions(s);
    s.page = Page::Settings;
}
pub(super) fn pending(s: &mut Desktop) {
    actions(s);
    let mut action = capability();
    action.status = "pending".into();
    action.manifest.title = "Review activation".into();
    s.snapshot = Some(Snapshot {
        capabilities: vec![action.clone()],
        jobs: vec![],
    });
    s.select(action);
}
pub(super) fn paired(s: &mut Desktop) {
    actions(s);
    s.connection = Connection::Paired("/synthetic/xlatch-client/config.toml".into());
    s.address
        .replace_text(s.connection.path().to_string_lossy().into_owned());
}
pub(super) fn offline(s: &mut Desktop) {
    actions(s);
    s.snapshot = None;
    s.error=Some("Synthetic offline state. Refresh or review the connection; no service is started automatically.".into());
}

pub(super) fn selected(s: &mut Desktop) {
    actions(s);
    s.select(capability());
}
pub(super) fn busy(s: &mut Desktop) {
    selected(s);
    s.busy = true;
}
pub(super) fn unknown(s: &mut Desktop) {
    selected(s);
    s.payload.replace_text("Synthetic uncertain request");
    let input = json!({"text":s.payload.text,"mime_type":"text/plain"}).to_string();
    s.draft = Draft::prepare(&capability(), &input, None).ok();
    s.error = Some(
        "Synthetic acknowledgement lost. Retry the same run without changing content or key."
            .into(),
    );
}
pub(super) fn empty(s: &mut Desktop) {
    actions(s);
    s.snapshot = Some(Snapshot {
        capabilities: vec![],
        jobs: vec![],
    });
}
pub(super) fn long(s: &mut Desktop) {
    selected(s);
    s.payload
        .replace_text("Synthetic long input for scrolling.\n".repeat(400));
    s.show_contract = false;
}
pub(super) fn contract(s: &mut Desktop) {
    selected(s);
    s.show_contract = true;
}

pub(super) fn result(s: &mut Desktop) {
    activity(s);
    s.show_result = true;
    s.result = serde_json::to_value(snapshot().jobs[2].clone()).ok();
}

pub(super) fn request(s: &mut Desktop, request: Option<Control>) {
    s.busy = false;
    if s.snapshot.is_none() {
        s.error = Some("Synthetic offline fixture; no network was attempted.".into());
        return;
    }
    let value = match request {
        None => return,
        Some(Control::Rpc {
            request:
                Request::Invoke {
                    capability_id,
                    revision,
                    input,
                    idempotency_key,
                },
        }) => {
            let job = Job {
                id: idempotency_key,
                capability_id,
                revision,
                input: input.clone(),
                result: Some(input),
                owner: "synthetic-review".into(),
                status: "succeeded".into(),
                error: None,
                created_at: 0,
            };
            if let Some(snapshot) = &mut s.snapshot {
                if let Some(existing) = snapshot.jobs.iter().find(|existing| existing.id == job.id)
                {
                    serde_json::to_value(existing)
                } else {
                    snapshot.jobs.insert(0, job.clone());
                    serde_json::to_value(job)
                }
            } else {
                serde_json::to_value(job)
            }
        }
        Some(Control::Rpc {
            request: Request::Job { id },
        }) => serde_json::to_value(
            s.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.jobs.iter().find(|job| job.id == id)),
        ),
        Some(Control::Rpc {
            request: Request::Cancel { id },
        }) => {
            if let Some(job) = s
                .snapshot
                .as_mut()
                .and_then(|snapshot| snapshot.jobs.iter_mut().find(|job| job.id == id))
            {
                if matches!(job.status.as_str(), "queued" | "running") {
                    job.status = "cancelled".into();
                }
                serde_json::to_value(job)
            } else {
                Ok(Value::Null)
            }
        }
        _ => {
            s.error=Some("Synthetic protected fixture: activation requires enrolled-phone approval. No authority was granted.".into());
            return;
        }
    };
    match value {
        Ok(value) => {
            s.result = Some(value);
            s.error = None;
        }
        Err(error) => s.error = Some(error.to_string()),
    }
}
