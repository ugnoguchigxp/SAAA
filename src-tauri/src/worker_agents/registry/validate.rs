//! Draft validation. Every rule here is enforced again at approval time on the stored rows, so a
//! row inserted by any other path cannot become executable without passing the same checks.
use crate::worker_agents::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};

pub(super) const MAX_SKILLS: usize = 8;
pub(super) const MAX_TOOLS: usize = 8;
const MAX_SCHEMA_BYTES: usize = 65_536;

fn valid_profile_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (3..=40).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// Compiles with the `jsonschema` crate. The crate is built without network/file retrievers, so
/// an external `$ref` cannot be resolved and fails here instead of fetching anything.
fn check_schema(label: &str, schema: &serde_json::Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err(format!("{label} must be a JSON object"));
    }
    if schema.to_string().len() > MAX_SCHEMA_BYTES {
        return Err(format!("{label} is too large"));
    }
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|_| format!("{label} is not a valid JSON Schema"))
}

/// Resolves the effect of one tool reference from the trusted side. User input never carries an
/// effect: builtin tools are host constants and catalog tools are read from the pinned
/// `tool_selection_revisions` row (which must belong to an enabled tool of an enabled source).
pub(super) fn resolve_tool_effect(
    connection: &Connection,
    tool: &ToolRef,
) -> Result<ToolEffect, String> {
    match tool.kind {
        ToolRefKind::Builtin => {
            if tool.catalog_revision_id.is_some() {
                return Err(format!(
                    "builtin tool {} cannot pin a catalog revision",
                    tool.key
                ));
            }
            if !BUILTIN_TOOL_KEYS.contains(&tool.key.as_str()) {
                return Err(format!("unknown builtin tool: {}", tool.key));
            }
            Ok(ToolEffect::Read)
        }
        ToolRefKind::Catalog => {
            if BUILTIN_TOOL_KEYS.contains(&tool.key.as_str()) {
                return Err(format!(
                    "catalog tool key collides with builtin: {}",
                    tool.key
                ));
            }
            let revision_id = tool
                .catalog_revision_id
                .as_deref()
                .ok_or_else(|| format!("catalog tool {} requires catalogRevisionId", tool.key))?;
            let effect: Option<String> = connection
                .query_row(
                    "SELECT r.effect FROM tool_selection_revisions r
                     JOIN tool_selection_catalog c ON c.id = r.tool_id
                     JOIN tool_selection_sources s ON s.id = c.source_id
                     WHERE r.id = ?1 AND c.id = ?2 AND c.enabled = 1 AND s.enabled = 1",
                    params![revision_id, tool.key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            let effect = effect.ok_or_else(|| format!("catalog tool unavailable: {}", tool.key))?;
            // An unparseable effect fails closed as `unknown`.
            Ok(ToolEffect::parse(&effect).unwrap_or(ToolEffect::Unknown))
        }
    }
}

/// Validates a draft and returns the resolved effect of each tool, in order.
pub(crate) fn validate_draft(
    connection: &Connection,
    draft: &ProfileDraft,
) -> Result<Vec<ToolEffect>, String> {
    if !valid_profile_id(&draft.profile_id) {
        return Err("profile_id must match ^[a-z][a-z0-9_]{2,39}$".into());
    }
    if draft.purpose.trim().is_empty() || draft.purpose.len() > 2000 {
        return Err("purpose must be 1..=2000 bytes".into());
    }
    if draft.system_context.trim().is_empty() || draft.system_context.len() > 16_384 {
        return Err("system_context must be 1..=16384 bytes".into());
    }
    if draft.tools.is_empty() || draft.tools.len() > MAX_TOOLS {
        return Err(format!("tools must have 1..={MAX_TOOLS} entries"));
    }
    let mut keys = std::collections::HashSet::new();
    let mut effects = Vec::with_capacity(draft.tools.len());
    for tool in &draft.tools {
        if tool.key.is_empty() || tool.key.len() > 256 {
            return Err("tool key must be 1..=256 bytes".into());
        }
        if !keys.insert(tool.key.as_str()) {
            return Err(format!("duplicate tool key: {}", tool.key));
        }
        effects.push(resolve_tool_effect(connection, tool)?);
    }
    if draft.skill_revision_ids.len() > MAX_SKILLS {
        return Err(format!("at most {MAX_SKILLS} skills are allowed"));
    }
    let mut skills = std::collections::HashSet::new();
    for skill in &draft.skill_revision_ids {
        if !skills.insert(skill.as_str()) {
            return Err(format!("duplicate skill revision: {skill}"));
        }
        let exists = connection
            .query_row(
                "SELECT 1 FROM worker_skill_revisions WHERE id = ?1",
                params![skill],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .is_some();
        if !exists {
            return Err(format!("unknown skill revision: {skill}"));
        }
    }
    check_schema("input_schema", &draft.input_schema)?;
    match (draft.output_kind, &draft.output_schema) {
        (OutputKind::JsonV1, Some(schema)) => check_schema("output_schema", schema)?,
        (OutputKind::JsonV1, None) => return Err("json_v1 requires output_schema".into()),
        (OutputKind::WebClaimsV1, Some(_)) => {
            return Err("web_claims_v1 does not take an output_schema".into())
        }
        (OutputKind::WebClaimsV1, None) => {
            if !draft.completion.sources_must_be_host_recorded {
                return Err("web_claims_v1 requires sources_must_be_host_recorded".into());
            }
            if draft.completion.min_items > 8 {
                return Err("web_claims_v1 min_items must be <= 8".into());
            }
        }
    }
    let limits = &draft.limits;
    if !(1..=20).contains(&limits.max_steps) {
        return Err("max_steps must be 1..=20".into());
    }
    if !(1_000..=300_000).contains(&limits.deadline_ms) {
        return Err("deadline_ms must be 1000..=300000".into());
    }
    if limits.sync_wait_ms > limits.deadline_ms {
        return Err("sync_wait_ms must not exceed deadline_ms".into());
    }
    if limits.max_same_tier_retries > 5 || limits.max_restarts > 3 {
        return Err("retry and restart limits are out of range".into());
    }
    Ok(effects)
}
