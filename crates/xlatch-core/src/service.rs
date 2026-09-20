//! Transport-neutral request dispatch.

use crate::{
    capability::{Job, Request},
    store::Store,
};
use anyhow::Result;
use serde_json::Value;

/// Dispatch an already authenticated request under its principal.
///
/// # Errors
/// Propagates validation, permission and storage errors.
pub fn dispatch(store: &mut Store, owner: &str, request: Request) -> Result<Value> {
    store.authorize_request(owner, &request)?;
    match request {
        Request::Approval { request } => crate::approval::dispatch(store, owner, request),
        Request::Enrollment { request } => crate::enrollment::dispatch(store, owner, request),
        Request::ChainCandidates { steps } => Ok(serde_json::to_value(
            store.chain_candidates(owner, &steps)?,
        )?),
        Request::InvokeChain {
            steps,
            input,
            idempotency_key,
        } => job_response(store.invoke_chain(owner, &steps, &input, &idempotency_key)?),
        Request::SaveChain { steps, title } => Ok(serde_json::to_value(
            store.save_chain(owner, &steps, &title)?,
        )?),
        Request::Discover => Ok(serde_json::to_value(store.discover(owner)?)?),
        Request::Invoke {
            capability_id,
            revision,
            input,
            idempotency_key,
        } => job_response(store.invoke(
            owner,
            &capability_id,
            &revision,
            &input,
            &idempotency_key,
        )?),
        Request::Jobs => Ok(serde_json::to_value(store.jobs(owner)?)?),
        Request::Job { id } => {
            let mut value = job_response(store.job(owner, &id)?)?;
            let steps = store.composition_steps(owner, &id)?;
            if !steps.is_empty() {
                value["steps"] = serde_json::to_value(steps)?;
            }
            Ok(value)
        }
        Request::Cancel { id } => job_response(store.cancel(owner, &id)?),
        Request::Events { after } => Ok(serde_json::to_value(store.events(owner, after)?)?),
    }
}

fn job_response(mut job: Job) -> Result<Value> {
    // Do not duplicate potentially large uploaded artifacts in every result response.
    job.input = Value::Null;
    Ok(serde_json::to_value(job)?)
}
