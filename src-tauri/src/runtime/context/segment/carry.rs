use serde_json::{json, Value};

pub(crate) struct Carry {
    pub(crate) body: Value,
    pub(crate) bytes: usize,
}

pub(crate) fn build_carry(evidence_refs: &[String], active_operations: &[String]) -> Result<Carry, String> {
    let mut refs = evidence_refs.to_vec();
    let mut body = json!({
        "scope_refs": [],
        "constraints": [],
        "active_operations": active_operations,
        "adopted_evidence_refs": refs,
        "omitted_history_locator": null,
        "goal": null,
        "target": null,
        "completion_criteria": null,
        "decisions": null,
        "open_items": null,
        "unresolved_conflicts": null,
        "status": "not_extracted"
    });
    while body.to_string().len() > 8_192 && !refs.is_empty() {
        refs.pop();
        body["adopted_evidence_refs"] = json!(refs);
    }
    if body.to_string().len() > 8_192 {
        body["constraints"] = json!([]);
        body["scope_refs"] = json!([]);
        body["omitted_history_locator"] = json!(null);
    }
    let bytes = body.to_string().len();
    if bytes > 8_192 {
        return Err("required_context_overflow: active_operations exceed carry budget".into());
    }
    Ok(Carry { body, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_43_carry_drops_evidence_refs_first() {
        let refs = (0..200).map(|index| format!("evidence-{index}-{}", "x".repeat(40))).collect::<Vec<_>>();
        let carry = build_carry(&refs, &["op".into()]).unwrap();
        assert!(carry.body["adopted_evidence_refs"].as_array().unwrap().len() < refs.len());
        assert_eq!(carry.body["active_operations"][0], "op");
    }

    #[test]
    fn cw_43_carry_fails_when_active_operations_exceed() {
        let ops = (0..400).map(|index| format!("operation-{index}-{}", "y".repeat(40))).collect::<Vec<_>>();
        let error = build_carry(&[], &ops).unwrap_err();
        assert!(error.contains("active_operations"));
    }
}
