//! Name normalization and semantic-key computation (WM-01).

use sha2::{Digest, Sha256};

/// Trim, collapse consecutive Unicode whitespace to one ASCII space,
/// lowercase ASCII A-Z only. Display value is never rewritten; this is
/// comparison-only.
pub fn normalize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    let mut started = false;
    for ch in raw.trim().chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && started {
            out.push(' ');
        }
        pending_space = false;
        started = true;
        if ch.is_ascii_uppercase() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn hex_id(input: &[u8]) -> String {
    format!("{:x}", Sha256::digest(input))
}

pub fn entity_key(project_scope: &str, entity_id: &str) -> String {
    let input = serde_json::json!(["entity", project_scope, entity_id]);
    format!(
        "{}{}",
        super::model::WORLD_SEMANTIC_KEY_PREFIX,
        hex_id(&serde_json::to_vec(&input).expect("json"))
    )
}

/// Inputs to `relation_key`. Grouped so the fixed array order stays reviewable.
pub struct RelationKeyInput<'a> {
    pub project_scope: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub relation_type: &'a str,
    pub effect_input: Option<&'a str>,
    pub sorted_conditions: &'a [(String, String)],
    pub valid_from: i64,
    pub valid_until: Option<i64>,
}

pub fn relation_key(input: &RelationKeyInput<'_>) -> String {
    let cond: Vec<(&str, &str)> = input
        .sorted_conditions
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let payload = serde_json::json!([
        "relation",
        input.project_scope,
        input.from,
        input.to,
        input.relation_type,
        input.effect_input,
        cond,
        input.valid_from,
        input.valid_until
    ]);
    format!(
        "{}{}",
        super::model::WORLD_SEMANTIC_KEY_PREFIX,
        hex_id(&serde_json::to_vec(&payload).expect("json"))
    )
}

pub fn focus_key(project_scope: &str, entity_id: &str, reason: &str) -> String {
    let input = serde_json::json!(["focus", project_scope, entity_id, reason]);
    format!(
        "{}{}",
        super::model::WORLD_SEMANTIC_KEY_PREFIX,
        hex_id(&serde_json::to_vec(&input).expect("json"))
    )
}

/// `related_to` is undirected: sort endpoints before keying.
pub fn order_undirected(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}
