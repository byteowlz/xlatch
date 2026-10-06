//! Explicit linear composition of independently registered capability revisions.
use crate::capability::{Execution, Manifest};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A reviewed step embeds its exact contract, making consent independent of registry labels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Snapshot of an independently registered leaf capability.
    pub manifest: Manifest,
    /// Expected digest of that snapshot.
    pub revision: String,
    /// Input wiring from the original input or previous result.
    pub input: Binding,
}
/// One independently reviewed fan-out destination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Snapshot of an independently registered leaf capability.
    pub manifest: Manifest,
    /// Expected digest of that snapshot.
    pub revision: String,
}
/// Wiring contains data only: no scripts, expressions, implicit conversions or routing heuristics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Binding {
    /// First step receives original input; later steps receive the previous result unchanged.
    Previous,
    /// Explicit top-level fields, checked against the next input schema at runtime.
    Fields {
        /// Destination field names mapped to explicit value sources.
        fields: BTreeMap<String, Source>,
    },
}
/// Explicit source of one mapped field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// JSON Pointer into the preceding result (original input for the first step).
    Previous {
        /// RFC 6901 pointer; empty means the entire document.
        pointer: String,
    },
    /// JSON Pointer into the original composition invocation input.
    Original {
        /// RFC 6901 pointer; empty means the entire document.
        pointer: String,
    },
    /// Fixed reviewed JSON value.
    Literal {
        /// Constant included in the composition revision.
        value: Value,
    },
}
impl Binding {
    /// Apply explicit wiring. Missing fields never turn into implicit null values.
    /// # Errors
    /// Returns invalid or missing JSON pointer errors.
    pub fn apply(&self, original: &Value, previous: &Value) -> Result<Value> {
        match self {
            Self::Previous => Ok(previous.clone()),
            Self::Fields { fields } => {
                let mapped = fields
                    .iter()
                    .map(|(name, source)| {
                        let value = match source {
                            Source::Previous { pointer } => previous
                                .pointer(pointer)
                                .with_context(|| format!("previous result has no {pointer}"))?,
                            Source::Original { pointer } => original
                                .pointer(pointer)
                                .with_context(|| format!("original input has no {pointer}"))?,
                            Source::Literal { value } => value,
                        };
                        Ok((name.clone(), value.clone()))
                    })
                    .collect::<Result<serde_json::Map<_, _>>>()?;
                Ok(Value::Object(mapped))
            }
        }
    }
}
/// Check composition shape without opening host files or reading approval state.
/// # Errors
/// Rejects nested compositions, inconsistent snapshots and unproven implicit connections.
pub fn validate(manifest: &Manifest, steps: &[Step]) -> Result<()> {
    ensure!(
        (2..=16).contains(&steps.len()),
        "compose requires 2–16 leaf steps"
    );
    let mut previous = &manifest.input_schema;
    let mut timeout = 0;
    for step in steps {
        ensure!(
            step.manifest.id != manifest.id
                && !matches!(step.manifest.execution, Execution::Compose { .. }),
            "nested or cyclic compositions are not supported"
        );
        step.manifest.validate()?;
        ensure!(
            step.manifest.revision()? == step.revision,
            "step snapshot digest mismatch"
        );
        match &step.input {
            Binding::Previous => ensure!(
                compatible(previous, &step.manifest.input_schema),
                "cannot prove direct input compatibility for {}; supply an explicit fields mapping (runtime checked)",
                step.manifest.id
            ),
            Binding::Fields { fields } => {
                ensure!(!fields.is_empty() && fields.len() <= 64, "map 1–64 fields");
                for source in fields.values() {
                    if let Source::Previous { pointer } | Source::Original { pointer } = source {
                        ensure!(
                            pointer.is_empty() || pointer.starts_with('/'),
                            "mapping requires a JSON Pointer"
                        );
                    }
                }
            }
        }
        previous = &step.manifest.output_schema;
        timeout += step.manifest.timeout_seconds;
    }
    ensure!(
        timeout <= manifest.timeout_seconds,
        "composition timeout must cover all step timeouts"
    );
    Ok(())
}
/// Validate independent destinations that all receive the original input.
pub fn validate_fan_out(manifest: &Manifest, targets: &[Target]) -> Result<()> {
    ensure!(
        (2..=16).contains(&targets.len()),
        "fan_out requires 2–16 leaf targets"
    );
    let mut timeout = 0;
    for target in targets {
        ensure!(
            target.manifest.id != manifest.id
                && !matches!(
                    target.manifest.execution,
                    Execution::Compose { .. } | Execution::FanOut { .. }
                ),
            "nested or cyclic compositions are not supported"
        );
        target.manifest.validate()?;
        ensure!(
            target.manifest.revision()? == target.revision,
            "target snapshot digest mismatch"
        );
        timeout = timeout.max(target.manifest.timeout_seconds);
    }
    ensure!(
        manifest.input_schema == fan_out_input_schema(targets),
        "fan-out input schema must bind every target"
    );
    ensure!(
        manifest.output_schema == fan_out_output_schema(),
        "fan-out output schema must describe branch receipts"
    );
    ensure!(
        manifest.accepts == fan_out_accepts(targets),
        "fan-out MIME types must be accepted by every target"
    );
    ensure!(
        timeout <= manifest.timeout_seconds,
        "fan-out timeout must cover every target timeout"
    );
    Ok(())
}
/// Exact conjunction used by a fan-out parent's public input contract.
#[must_use]
pub fn fan_out_input_schema(targets: &[Target]) -> Value {
    json!({"allOf": targets.iter().map(|target| target.manifest.input_schema.clone()).collect::<Vec<_>>()})
}
/// Stable aggregate result contract for fan-out parents.
#[must_use]
pub fn fan_out_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["results"],
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["capability_id", "revision", "result"],
                    "properties": {
                        "capability_id": {"type": "string"},
                        "revision": {"type": "string"},
                        "result": true
                    }
                }
            }
        }
    })
}
/// MIME patterns accepted by every target, preserving the narrowest matching pattern.
#[must_use]
pub fn fan_out_accepts(targets: &[Target]) -> Vec<String> {
    fn intersection(left: &str, right: &str) -> Option<String> {
        let (left_type, left_subtype) = left.split_once('/')?;
        let (right_type, right_subtype) = right.split_once('/')?;
        let media_type = match (left_type, right_type) {
            ("*", value) | (value, "*") => value,
            (a, b) if a == b => a,
            _ => return None,
        };
        let subtype = match (left_subtype, right_subtype) {
            ("*", value) | (value, "*") => value,
            (a, b) if a == b => a,
            _ => return None,
        };
        Some(format!("{media_type}/{subtype}"))
    }
    let Some(first) = targets.first() else {
        return Vec::new();
    };
    let mut accepted = first.manifest.accepts.clone();
    for target in &targets[1..] {
        accepted = accepted
            .iter()
            .flat_map(|left| {
                target
                    .manifest
                    .accepts
                    .iter()
                    .filter_map(|right| intersection(left, right))
            })
            .collect();
        accepted.sort();
        accepted.dedup();
    }
    accepted
}
/// Conservative proof for ordinary object/scalar contracts; unfamiliar constraints need explicit wiring.
#[must_use]
pub fn compatible(source: &Value, target: &Value) -> bool {
    if source == target || *target == json!(true) || *target == json!({}) {
        return true;
    }
    if let Some(constant) = source.get("const") {
        return jsonschema::validator_for(target).is_ok_and(|v| v.is_valid(constant));
    }
    let Some(target) = target.as_object() else {
        return false;
    };
    if target
        .get("additionalProperties")
        .is_some_and(|v| !v.is_boolean())
    {
        return false;
    }
    if target.keys().any(|k| {
        ![
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
            "title",
        ]
        .contains(&k.as_str())
    }) {
        return false;
    }
    if target.get("type") != source.get("type") {
        return false;
    }
    if target.get("type") != Some(&json!("object")) {
        return target
            .keys()
            .all(|k| ["type", "description", "title"].contains(&k.as_str()));
    }
    let required = source.get("required").and_then(Value::as_array);
    if target
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|keys| {
            keys.iter()
                .any(|k| !required.is_some_and(|r| r.contains(k)))
        })
    {
        return false;
    }
    let properties = source.get("properties").and_then(Value::as_object);
    let target_properties = target.get("properties").and_then(Value::as_object);
    if target.get("additionalProperties") == Some(&json!(false))
        && (source.get("additionalProperties") != Some(&json!(false))
            || properties.is_some_and(|p| {
                p.keys()
                    .any(|k| !target_properties.is_some_and(|t| t.contains_key(k)))
            }))
    {
        return false;
    }
    target_properties.is_none_or(|props| {
        props.iter().all(|(name, schema)| {
            properties.and_then(|p| p.get(name)).map_or_else(
                || source.get("additionalProperties") == Some(&json!(false)),
                |s| compatible(s, schema),
            )
        })
    })
}
