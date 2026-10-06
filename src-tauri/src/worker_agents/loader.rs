//! Read-side helpers shared by every workstream: epochs, mode, and immutable revision loading.
use super::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Meta {
    pub registry_epoch: i64,
    pub acl_epoch: i64,
    pub web_search_mode: WebSearchMode,
}

pub(crate) fn read_meta(connection: &Connection) -> Result<Meta, String> {
    connection
        .query_row(
            "SELECT registry_epoch, acl_epoch, web_search_mode FROM worker_meta WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|error| format!("worker meta unavailable: {error}"))
        .and_then(|(registry_epoch, acl_epoch, mode)| {
            Ok(Meta {
                registry_epoch,
                acl_epoch,
                web_search_mode: WebSearchMode::parse(&mode)
                    .ok_or_else(|| "worker meta has an unknown web_search_mode".to_string())?,
            })
        })
}

/// sha256 over the normalized draft. Approval binds to this hash, so what the user reviewed is
/// exactly what runs.
pub(crate) fn definition_hash(draft: &ProfileDraft) -> String {
    let normalized = serde_json::to_string(draft).unwrap_or_default();
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

fn invalid(what: &str) -> String {
    format!("worker revision has an invalid {what}")
}

/// Loads one revision with its skills and tools. Does not check review state: callers that
/// execute must require `ReviewState::Approved`.
pub(crate) fn load_revision(
    connection: &Connection,
    revision_id: &str,
) -> Result<LoadedRevision, String> {
    let row = connection
        .query_row(
            "SELECT profile_id, revision, review_state, purpose, system_context, definition_hash,
                    input_schema_json, output_kind, output_schema_json, completion_json,
                    limits_json, tier_policy_json
             FROM worker_profile_revisions WHERE id = ?1",
            params![revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "worker revision not found".to_string())?;
    let (
        profile_id,
        revision,
        review_state,
        purpose,
        system_context,
        definition_hash,
        input_schema,
        output_kind,
        output_schema,
        completion,
        limits,
        tier_policy,
    ) = row;

    let skills = {
        let mut statement = connection
            .prepare(
                "SELECT s.name, r.body FROM worker_profile_skills p
                 JOIN worker_skill_revisions r ON r.id = p.skill_revision_id
                 JOIN worker_skills s ON s.id = r.skill_id
                 WHERE p.profile_revision_id = ?1 ORDER BY p.ordinal",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![revision_id], |row| {
                Ok(LoadedSkill {
                    name: row.get(0)?,
                    body: row.get(1)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let tools = {
        let mut statement = connection
            .prepare(
                "SELECT tool_kind, tool_key, catalog_revision_id, effect FROM worker_profile_tools
                 WHERE profile_revision_id = ?1 ORDER BY ordinal",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![revision_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        let mut tools = Vec::new();
        for item in rows {
            let (kind, key, catalog_revision_id, effect) =
                item.map_err(|error| error.to_string())?;
            tools.push(LoadedTool {
                kind: match kind.as_str() {
                    "builtin" => ToolRefKind::Builtin,
                    "catalog" => ToolRefKind::Catalog,
                    _ => return Err(invalid("tool kind")),
                },
                key,
                catalog_revision_id,
                effect: ToolEffect::parse(&effect).ok_or_else(|| invalid("tool effect"))?,
            });
        }
        tools
    };
    Ok(LoadedRevision {
        profile_id,
        revision_id: revision_id.to_string(),
        revision,
        review_state: ReviewState::parse(&review_state).ok_or_else(|| invalid("review state"))?,
        purpose,
        system_context,
        skills,
        tools,
        input_schema: serde_json::from_str(&input_schema).map_err(|_| invalid("input schema"))?,
        output_kind: OutputKind::parse(&output_kind).ok_or_else(|| invalid("output kind"))?,
        output_schema: output_schema
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|_| invalid("output schema"))?,
        completion: serde_json::from_str(&completion).map_err(|_| invalid("completion"))?,
        limits: serde_json::from_str(&limits).map_err(|_| invalid("limits"))?,
        tier_policy: serde_json::from_str(&tier_policy).map_err(|_| invalid("tier policy"))?,
        definition_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_agents::test_support::*;

    #[test]
    fn loads_an_inserted_revision_with_its_tools() {
        let connection = fresh_db();
        let draft = sample_draft("alpha_agent", "answers questions about alpha");
        let revision_id = insert_revision(&connection, &draft, ReviewState::Approved, true);
        let loaded = load_revision(&connection, &revision_id).unwrap();
        assert_eq!(loaded.profile_id, "alpha_agent");
        assert_eq!(loaded.review_state, ReviewState::Approved);
        assert_eq!(loaded.tools.len(), 1);
        assert!(loaded.all_tools_read_only());
        assert_eq!(loaded.definition_hash, definition_hash(&draft));
    }

    #[test]
    fn unknown_revision_is_an_error() {
        assert!(load_revision(&fresh_db(), "missing").is_err());
    }

    #[test]
    fn definition_hash_changes_with_the_definition() {
        let mut draft = sample_draft("alpha_agent", "purpose");
        let first = definition_hash(&draft);
        draft.system_context.push('!');
        assert_ne!(first, definition_hash(&draft));
    }

    #[test]
    fn meta_defaults_to_inline_mode() {
        let meta = read_meta(&fresh_db()).unwrap();
        assert_eq!(meta.web_search_mode, WebSearchMode::Inline);
        assert_eq!(meta.registry_epoch, 0);
    }
}
