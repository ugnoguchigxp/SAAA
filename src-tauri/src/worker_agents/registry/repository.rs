//! Registry writes. Pure functions over `&Connection`; callers supply the transaction (the
//! IPC wrappers use `SqliteWriter::transact`). Multi-statement operations additionally run in a
//! SAVEPOINT so they are atomic whether or not the caller already opened a transaction.
use super::queries::{get_summary, revision_summary};
use super::validate::validate_draft;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::definition_hash;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

const MAX_REVISIONS_PER_PROFILE: i64 = 500;
const MAX_EMBEDDING_DIMENSION: usize = 8192;

pub(super) fn atomically<T>(
    connection: &Connection,
    operation: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    connection
        .execute_batch("SAVEPOINT worker_registry")
        .map_err(|error| error.to_string())?;
    match operation(connection) {
        Ok(value) => {
            connection
                .execute_batch("RELEASE worker_registry")
                .map_err(|error| error.to_string())?;
            Ok(value)
        }
        Err(error) => {
            let _ =
                connection.execute_batch("ROLLBACK TO worker_registry; RELEASE worker_registry");
            Err(error)
        }
    }
}

fn db(error: rusqlite::Error) -> String {
    error.to_string()
}

fn bump_registry_epoch(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            "UPDATE worker_meta SET registry_epoch = registry_epoch + 1 WHERE singleton = 1",
            [],
        )
        .map(|_| ())
        .map_err(db)
}

fn insert_fts(
    connection: &Connection,
    revision_id: &str,
    profile_id: &str,
    purpose: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO worker_profile_fts(revision_id, search_text) VALUES(?1, ?2)",
            params![revision_id, format!("{profile_id} {purpose}")],
        )
        .map(|_| ())
        .map_err(db)
}

fn remove_index_rows(connection: &Connection, revision_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM worker_profile_fts WHERE revision_id = ?1",
            params![revision_id],
        )
        .map_err(db)?;
    connection
        .execute(
            "DELETE FROM worker_profile_embeddings WHERE revision_id = ?1",
            params![revision_id],
        )
        .map_err(db)?;
    Ok(())
}

struct NewRevision<'a> {
    draft: &'a ProfileDraft,
    effects: &'a [ToolEffect],
    revision_id: &'a str,
    revision: i64,
    state: ReviewState,
    created_by: &'a str,
    now_ms: i64,
}

fn insert_revision_rows(connection: &Connection, new: &NewRevision<'_>) -> Result<(), String> {
    let draft = new.draft;
    connection
        .execute(
            "INSERT INTO worker_profile_revisions(id, profile_id, revision, review_state, created_by,
                purpose, system_context, definition_hash, input_schema_json, output_kind,
                output_schema_json, completion_json, limits_json, tier_policy_json,
                created_at_ms, approved_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                new.revision_id,
                draft.profile_id,
                new.revision,
                new.state.as_str(),
                new.created_by,
                draft.purpose,
                draft.system_context,
                definition_hash(draft),
                draft.input_schema.to_string(),
                draft.output_kind.as_str(),
                draft.output_schema.as_ref().map(|schema| schema.to_string()),
                serde_json::to_string(&draft.completion).map_err(|e| e.to_string())?,
                serde_json::to_string(&draft.limits).map_err(|e| e.to_string())?,
                serde_json::to_string(&draft.tier_policy).map_err(|e| e.to_string())?,
                new.now_ms,
                (new.state == ReviewState::Approved).then_some(new.now_ms),
            ],
        )
        .map_err(db)?;
    for (ordinal, (tool, effect)) in draft.tools.iter().zip(new.effects).enumerate() {
        connection
            .execute(
                "INSERT INTO worker_profile_tools(profile_revision_id, ordinal, tool_kind, tool_key,
                    catalog_revision_id, effect) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new.revision_id,
                    ordinal as i64,
                    match tool.kind {
                        ToolRefKind::Builtin => "builtin",
                        ToolRefKind::Catalog => "catalog",
                    },
                    tool.key,
                    tool.catalog_revision_id,
                    effect.as_str(),
                ],
            )
            .map_err(db)?;
    }
    for (ordinal, skill_revision_id) in draft.skill_revision_ids.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO worker_profile_skills(profile_revision_id, ordinal, skill_revision_id)
                 VALUES(?1,?2,?3)",
                params![new.revision_id, ordinal as i64, skill_revision_id],
            )
            .map_err(db)?;
    }
    Ok(())
}

/// Creates a new `draft` revision. Does not touch `current_revision_id` or `registry_epoch`: a
/// draft is neither discoverable nor executable until the user approves its exact hash.
pub(crate) fn save_draft(
    connection: &Connection,
    draft: &ProfileDraft,
    created_by: &str,
    now_ms: i64,
) -> Result<RevisionSummary, String> {
    if created_by != "user_ipc" && created_by != "host_seed" {
        return Err("invalid created_by".into());
    }
    atomically(connection, |connection| {
        let effects = validate_draft(connection, draft)?;
        let hash = definition_hash(draft);
        // Saving an identical definition again is a no-op, not a new revision.
        let existing: Option<String> = connection
            .query_row(
                "SELECT id FROM worker_profile_revisions
                 WHERE profile_id = ?1 AND definition_hash = ?2 AND review_state = 'draft'",
                params![draft.profile_id, hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        if let Some(revision_id) = existing {
            return revision_summary(connection, &revision_id);
        }
        connection
            .execute(
                "INSERT OR IGNORE INTO worker_profiles(id, origin, enabled, pinned_offer,
                    current_revision_id, created_at_ms, updated_at_ms)
                 VALUES(?1, 'user', 1, 0, NULL, ?2, ?2)",
                params![draft.profile_id, now_ms],
            )
            .map_err(db)?;
        let (revision, count): (i64, i64) = connection
            .query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1, COUNT(*) FROM worker_profile_revisions
                 WHERE profile_id = ?1",
                params![draft.profile_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db)?;
        if count >= MAX_REVISIONS_PER_PROFILE {
            return Err("too many revisions for this worker agent".into());
        }
        let revision_id = format!("wrev_{}", uuid::Uuid::new_v4().simple());
        insert_revision_rows(
            connection,
            &NewRevision {
                draft,
                effects: &effects,
                revision_id: &revision_id,
                revision,
                state: ReviewState::Draft,
                created_by,
                now_ms,
            },
        )?;
        revision_summary(connection, &revision_id)
    })
}

/// Rebuilds the draft a stored revision represents, from its rows only.
fn draft_from_rows(connection: &Connection, revision_id: &str) -> Result<ProfileDraft, String> {
    let corrupt = |what: &str| format!("stored worker revision has an invalid {what}");
    let (profile_id, purpose, system_context, input, kind, output, completion, limits, tier) =
        connection
            .query_row(
                "SELECT profile_id, purpose, system_context, input_schema_json, output_kind,
                        output_schema_json, completion_json, limits_json, tier_policy_json
                 FROM worker_profile_revisions WHERE id = ?1",
                params![revision_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .map_err(db)?;
    let mut tool_statement = connection
        .prepare(
            "SELECT tool_kind, tool_key, catalog_revision_id FROM worker_profile_tools
             WHERE profile_revision_id = ?1 ORDER BY ordinal",
        )
        .map_err(db)?;
    let mut tools = Vec::new();
    for item in tool_statement
        .query_map(params![revision_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db)?
    {
        let (tool_kind, key, catalog_revision_id) = item.map_err(db)?;
        tools.push(ToolRef {
            kind: match tool_kind.as_str() {
                "builtin" => ToolRefKind::Builtin,
                "catalog" => ToolRefKind::Catalog,
                _ => return Err(corrupt("tool kind")),
            },
            key,
            catalog_revision_id,
        });
    }
    let mut skill_statement = connection
        .prepare(
            "SELECT skill_revision_id FROM worker_profile_skills
             WHERE profile_revision_id = ?1 ORDER BY ordinal",
        )
        .map_err(db)?;
    let skill_revision_ids = skill_statement
        .query_map(params![revision_id], |row| row.get::<_, String>(0))
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    Ok(ProfileDraft {
        profile_id,
        purpose,
        system_context,
        skill_revision_ids,
        tools,
        input_schema: serde_json::from_str(&input).map_err(|_| corrupt("input schema"))?,
        output_kind: OutputKind::parse(&kind).ok_or_else(|| corrupt("output kind"))?,
        output_schema: output
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|_| corrupt("output schema"))?,
        completion: serde_json::from_str(&completion).map_err(|_| corrupt("completion"))?,
        limits: serde_json::from_str(&limits).map_err(|_| corrupt("limits"))?,
        tier_policy: serde_json::from_str(&tier).map_err(|_| corrupt("tier policy"))?,
    })
}

/// Approves one draft revision, binding the approval to `definition_hash` (what the user saw).
pub(crate) fn approve_revision(
    connection: &Connection,
    profile_id: &str,
    revision_id: &str,
    definition_hash_input: &str,
    now_ms: i64,
) -> Result<WorkerAgentSummary, String> {
    atomically(connection, |connection| {
        let (state, stored_hash, revision): (String, String, i64) = connection
            .query_row(
                "SELECT review_state, definition_hash, revision FROM worker_profile_revisions
                 WHERE id = ?1 AND profile_id = ?2",
                params![revision_id, profile_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(db)?
            .ok_or_else(|| "worker revision not found".to_string())?;
        if state != ReviewState::Draft.as_str() {
            return Err("revision_not_draft".into());
        }
        if stored_hash != definition_hash_input {
            return Err("definition_hash_mismatch".into());
        }
        let draft = draft_from_rows(connection, revision_id)?;
        if definition_hash(&draft) != stored_hash || draft.profile_id != profile_id {
            return Err("definition_hash_mismatch".into());
        }
        // Re-validate against the live catalog, then gate on the effects recorded at save time
        // and on those resolved now: either one being non-read-only keeps the revision a draft.
        let live_effects = validate_draft(connection, &draft)?;
        let mut stored_statement = connection
            .prepare("SELECT effect FROM worker_profile_tools WHERE profile_revision_id = ?1")
            .map_err(db)?;
        let stored_effects = stored_statement
            .query_map(params![revision_id], |row| row.get::<_, String>(0))
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        let stored_ok = stored_effects
            .iter()
            .all(|effect| ToolEffect::parse(effect).is_some_and(ToolEffect::is_read_only));
        if !stored_ok || !live_effects.iter().all(|effect| effect.is_read_only()) {
            return Err("write_tools_not_supported".into());
        }

        let previous: Option<String> = connection
            .query_row(
                "SELECT current_revision_id FROM worker_profiles WHERE id = ?1",
                params![profile_id],
                |row| row.get(0),
            )
            .map_err(db)?;
        if let Some(previous) = previous.filter(|previous| previous != revision_id) {
            connection
                .execute(
                    "UPDATE worker_profile_revisions SET review_state = 'superseded' WHERE id = ?1",
                    params![previous],
                )
                .map_err(db)?;
            remove_index_rows(connection, &previous)?;
        }
        // Older drafts can no longer be approved over this newer definition.
        let mut stale_statement = connection
            .prepare(
                "SELECT id FROM worker_profile_revisions
                 WHERE profile_id = ?1 AND review_state = 'draft' AND revision < ?2",
            )
            .map_err(db)?;
        let stale = stale_statement
            .query_map(params![profile_id, revision], |row| row.get::<_, String>(0))
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        for id in stale {
            connection
                .execute(
                    "UPDATE worker_profile_revisions SET review_state = 'superseded' WHERE id = ?1",
                    params![id],
                )
                .map_err(db)?;
        }
        connection
            .execute(
                "UPDATE worker_profile_revisions SET review_state = 'approved', approved_at_ms = ?2
                 WHERE id = ?1",
                params![revision_id, now_ms],
            )
            .map_err(db)?;
        connection
            .execute(
                "UPDATE worker_profiles SET current_revision_id = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![profile_id, revision_id, now_ms],
            )
            .map_err(db)?;
        remove_index_rows(connection, revision_id)?;
        insert_fts(connection, revision_id, profile_id, &draft.purpose)?;
        bump_registry_epoch(connection)?;
        get_summary(connection, profile_id)
    })
}

pub(crate) fn set_enabled(
    connection: &Connection,
    profile_id: &str,
    enabled: bool,
    now_ms: i64,
) -> Result<WorkerAgentSummary, String> {
    atomically(connection, |connection| {
        let changed = connection
            .execute(
                "UPDATE worker_profiles SET enabled = ?2, updated_at_ms = ?3 WHERE id = ?1",
                params![profile_id, enabled as i64, now_ms],
            )
            .map_err(db)?;
        if changed == 0 {
            return Err("worker agent not found".into());
        }
        bump_registry_epoch(connection)?;
        get_summary(connection, profile_id)
    })
}

/// Skill revisions are immutable: this only inserts. The same body for the same skill returns the
/// existing revision. A skill's name is fixed at creation because loaders join it at run time.
pub(crate) fn save_skill(
    connection: &Connection,
    draft: &SkillDraft,
    now_ms: i64,
) -> Result<SkillSaved, String> {
    if draft.name.trim().is_empty() || draft.name.chars().count() > 80 {
        return Err("skill name must be 1..=80 characters".into());
    }
    if draft.body.trim().is_empty() || draft.body.len() > 16_384 {
        return Err("skill body must be 1..=16384 bytes".into());
    }
    atomically(connection, |connection| {
        let skill_id = match &draft.skill_id {
            Some(skill_id) => {
                let name: Option<String> = connection
                    .query_row(
                        "SELECT name FROM worker_skills WHERE id = ?1",
                        params![skill_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(db)?;
                match name {
                    None => return Err("unknown skill".into()),
                    Some(name) if name != draft.name => {
                        return Err("skill_name_immutable".into());
                    }
                    Some(_) => skill_id.clone(),
                }
            }
            None => {
                let skill_id = format!("wskill_{}", uuid::Uuid::new_v4().simple());
                connection
                    .execute(
                        "INSERT INTO worker_skills(id, name) VALUES(?1, ?2)",
                        params![skill_id, draft.name],
                    )
                    .map_err(db)?;
                skill_id
            }
        };
        let content_hash = format!("{:x}", Sha256::digest(draft.body.as_bytes()));
        let existing: Option<String> = connection
            .query_row(
                "SELECT id FROM worker_skill_revisions WHERE skill_id = ?1 AND content_hash = ?2",
                params![skill_id, content_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(db)?;
        let revision_id = match existing {
            Some(revision_id) => revision_id,
            None => {
                let revision_id = format!("wskillrev_{}", uuid::Uuid::new_v4().simple());
                connection
                    .execute(
                        "INSERT INTO worker_skill_revisions(id, skill_id, content_hash, body, created_at_ms)
                         VALUES(?1,?2,?3,?4,?5)",
                        params![revision_id, skill_id, content_hash, draft.body, now_ms],
                    )
                    .map_err(db)?;
                revision_id
            }
        };
        Ok(SkillSaved {
            skill_id,
            revision_id,
        })
    })
}

/// No epoch change: the mode selects instructions and tools, not the registry contents.
pub(crate) fn set_web_search_mode(
    connection: &Connection,
    mode: WebSearchMode,
) -> Result<WebSearchMode, String> {
    connection
        .execute(
            "UPDATE worker_meta SET web_search_mode = ?1 WHERE singleton = 1",
            params![mode.as_str()],
        )
        .map_err(db)?;
    Ok(mode)
}

/// Stores the embedding of a (current) revision for discovery. Little-endian f32.
pub(crate) fn index_embedding(
    connection: &Connection,
    revision_id: &str,
    model_hash: &str,
    vector: &[f32],
) -> Result<(), String> {
    if model_hash.is_empty() || vector.is_empty() || vector.len() > MAX_EMBEDDING_DIMENSION {
        return Err("invalid embedding".into());
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err("embedding contains a non-finite value".into());
    }
    let exists = connection
        .query_row(
            "SELECT 1 FROM worker_profile_revisions WHERE id = ?1",
            params![revision_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(db)?
        .is_some();
    if !exists {
        return Err("worker revision not found".into());
    }
    let blob: Vec<u8> = vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    connection
        .execute(
            "INSERT OR REPLACE INTO worker_profile_embeddings(revision_id, model_hash, dimension, vector)
             VALUES(?1,?2,?3,?4)",
            params![revision_id, model_hash, vector.len() as i64, blob],
        )
        .map_err(db)?;
    Ok(())
}

/// Inserts host-defined profiles that do not exist yet as approved revision 1. A profile that
/// already has any row is left alone, so user edits are never overwritten. Idempotent.
pub(crate) fn seed_builtin(
    connection: &Connection,
    now_ms: i64,
    drafts: &[ProfileDraft],
) -> Result<(), String> {
    atomically(connection, |connection| {
        for draft in drafts {
            let exists = connection
                .query_row(
                    "SELECT 1 FROM worker_profiles WHERE id = ?1",
                    params![draft.profile_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(db)?
                .is_some();
            if exists {
                continue;
            }
            let effects = validate_draft(connection, draft)
                .map_err(|error| format!("builtin worker {}: {error}", draft.profile_id))?;
            let revision_id = format!("wrev_{}", uuid::Uuid::new_v4().simple());
            connection
                .execute(
                    "INSERT INTO worker_profiles(id, origin, enabled, pinned_offer, current_revision_id,
                        created_at_ms, updated_at_ms) VALUES(?1, 'builtin', 1, 1, NULL, ?2, ?2)",
                    params![draft.profile_id, now_ms],
                )
                .map_err(db)?;
            insert_revision_rows(
                connection,
                &NewRevision {
                    draft,
                    effects: &effects,
                    revision_id: &revision_id,
                    revision: 1,
                    state: ReviewState::Approved,
                    created_by: "host_seed",
                    now_ms,
                },
            )?;
            connection
                .execute(
                    "UPDATE worker_profiles SET current_revision_id = ?2 WHERE id = ?1",
                    params![draft.profile_id, revision_id],
                )
                .map_err(db)?;
            insert_fts(connection, &revision_id, &draft.profile_id, &draft.purpose)?;
            bump_registry_epoch(connection)?;
        }
        Ok(())
    })
}
