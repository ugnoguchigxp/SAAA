use super::{generation::GenerationHandle, source::Candidate};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Final transport validation.  Selection is not evidence of delivery: adapter wrappers and
/// follow-up rebuilding must still contain every required item in the actual provider body.
pub(crate) fn verify_required_wire(body: &Value, selected: &[Candidate]) -> Result<(), String> {
    for candidate in selected
        .iter()
        .filter(|candidate| candidate.requirement == super::source::Requirement::Must)
    {
        if !value_contains(body, &candidate.content) {
            return Err(format!(
                "required_context_missing_from_wire:{}",
                candidate.candidate_id
            ));
        }
    }
    Ok(())
}

fn value_contains(value: &Value, required: &str) -> bool {
    match value {
        Value::String(text) => text.contains(required),
        Value::Array(values) => values.iter().any(|value| value_contains(value, required)),
        Value::Object(values) => values.values().any(|value| value_contains(value, required)),
        _ => false,
    }
}

pub(crate) fn record(
    generation: &GenerationHandle,
    health: &str,
    selected: &[Candidate],
    omitted: &[Candidate],
    tools: &[Value],
    include_world: bool,
) -> Result<(), String> {
    super::world::source::reject_dispatch(selected, omitted)?;
    let (selected, omitted) = super::world::turn::for_record(selected, omitted, include_world);
    let required_set =
        super::required::RequiredContextSet::from_references(selected.iter().copied());
    generation.set_health(health)?;
    // This source-backed receipt links the generation's request digest with the exact required
    // candidate identities without persisting their text a second time.
    generation.add_input(
        "required-context-set",
        &required_set.digest,
        1,
        &required_set.digest,
        "must",
        "reference",
        true,
        None,
    )?;
    for (candidate, included, reason) in selected
        .iter()
        .map(|candidate| (*candidate, true, None))
        .chain(
            omitted
                .iter()
                .map(|candidate| (*candidate, false, Some("budget-or-policy"))),
        )
    {
        generation.add_input(
            &candidate.source_kind,
            &candidate.source_id,
            candidate.source_version,
            &candidate.source_digest,
            candidate.requirement.as_str(),
            candidate.placement.as_str(),
            included,
            reason,
        )?;
    }
    for tool in tools {
        let encoded = serde_json::to_vec(tool).map_err(|_| "Tool definition is invalid")?;
        let name = tool["function"]["name"]
            .as_str()
            .ok_or("Tool definition has no function name")?;
        generation.add_input(
            "static-tool-offer",
            name,
            1,
            &format!("{:x}", Sha256::digest(&encoded)),
            "should",
            "tool-schema",
            true,
            None,
        )?;
    }
    generation.dispatch()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::context::source::{Candidate, Requirement};
    use serde_json::json;

    fn required() -> Candidate {
        Candidate::untrusted(
            "required".into(),
            "personal-state",
            vec![],
            Requirement::Must,
            "state".into(),
            1,
            1,
            r#"{"status":"Active","value":"do not send"}"#.into(),
        )
    }

    #[test]
    fn wire_validation_requires_the_actual_selected_content() {
        let candidate = required();
        assert!(verify_required_wire(
            &json!({"messages":[{"content": format!("context {}", candidate.content)}]}),
            std::slice::from_ref(&candidate),
        )
        .is_ok());
        assert!(verify_required_wire(&json!({"messages":[]}), &[candidate])
            .unwrap_err()
            .contains("required_context_missing_from_wire"));
    }
}
