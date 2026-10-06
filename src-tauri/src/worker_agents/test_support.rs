//! Test fixtures shared by the worker-agent workstreams. No validation: tests insert exactly the
//! rows they need. Production writes go through `registry`.
use super::contracts::*;
use super::loader::{definition_hash, now_ms};
use rusqlite::{params, Connection};

/// A full production schema on an in-memory database (includes the primary conversation).
pub(crate) fn fresh_db() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    crate::persistence::schema::initialize_database(&connection).unwrap();
    connection
}

pub(crate) fn sample_draft(profile_id: &str, purpose: &str) -> ProfileDraft {
    ProfileDraft {
        profile_id: profile_id.into(),
        purpose: purpose.into(),
        system_context: "You are a narrow worker.".into(),
        skill_revision_ids: vec![],
        tools: vec![ToolRef {
            kind: ToolRefKind::Builtin,
            key: "web_search".into(),
            catalog_revision_id: None,
        }],
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
            "additionalProperties": false
        }),
        output_kind: OutputKind::JsonV1,
        output_schema: Some(serde_json::json!({"type": "object"})),
        completion: CompletionCriteria {
            min_items: 0,
            sources_must_be_host_recorded: false,
        },
        limits: WorkerLimits::default(),
        tier_policy: TierPolicy {
            max_tier: Tier::Local,
            cloud: CloudPolicy::Never,
        },
    }
}

/// Inserts profile (if missing) + one revision + tools + a trigram FTS row. Returns the revision
/// id. `make_current` points the profile at it (use only with `Approved`).
pub(crate) fn insert_revision(
    connection: &Connection,
    draft: &ProfileDraft,
    state: ReviewState,
    make_current: bool,
) -> String {
    let now = now_ms();
    connection
        .execute(
            "INSERT OR IGNORE INTO worker_profiles(id,origin,enabled,pinned_offer,current_revision_id,created_at_ms,updated_at_ms)
             VALUES(?1,'user',1,0,NULL,?2,?2)",
            params![draft.profile_id, now],
        )
        .unwrap();
    let revision: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(revision),0)+1 FROM worker_profile_revisions WHERE profile_id=?1",
            params![draft.profile_id],
            |row| row.get(0),
        )
        .unwrap();
    let revision_id = format!("wrev_{}_{revision}", draft.profile_id);
    connection
        .execute(
            "INSERT INTO worker_profile_revisions(id,profile_id,revision,review_state,created_by,purpose,system_context,
                definition_hash,input_schema_json,output_kind,output_schema_json,completion_json,limits_json,
                tier_policy_json,created_at_ms,approved_at_ms)
             VALUES(?1,?2,?3,?4,'user_ipc',?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                revision_id,
                draft.profile_id,
                revision,
                state.as_str(),
                draft.purpose,
                draft.system_context,
                definition_hash(draft),
                draft.input_schema.to_string(),
                draft.output_kind.as_str(),
                draft.output_schema.as_ref().map(|value| value.to_string()),
                serde_json::to_string(&draft.completion).unwrap(),
                serde_json::to_string(&draft.limits).unwrap(),
                serde_json::to_string(&draft.tier_policy).unwrap(),
                now,
                (state == ReviewState::Approved).then_some(now),
            ],
        )
        .unwrap();
    for (ordinal, tool) in draft.tools.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO worker_profile_tools(profile_revision_id,ordinal,tool_kind,tool_key,catalog_revision_id,effect)
                 VALUES(?1,?2,?3,?4,?5,'read')",
                params![
                    revision_id,
                    ordinal as i64,
                    match tool.kind {
                        ToolRefKind::Builtin => "builtin",
                        ToolRefKind::Catalog => "catalog",
                    },
                    tool.key,
                    tool.catalog_revision_id,
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO worker_profile_fts(revision_id, search_text) VALUES(?1, ?2)",
            params![
                revision_id,
                format!("{} {}", draft.profile_id, draft.purpose)
            ],
        )
        .unwrap();
    if make_current {
        connection
            .execute(
                "UPDATE worker_profiles SET current_revision_id=?2, updated_at_ms=?3 WHERE id=?1",
                params![draft.profile_id, revision_id, now],
            )
            .unwrap();
    }
    revision_id
}

/// Inserts a persisted user message and returns its id (admission binds to it).
pub(crate) fn insert_user_message(connection: &Connection, id: &str, content: &str) {
    connection
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES(?1, ?2, 'user', ?3, ?4)",
            params![
                id,
                crate::PRIMARY_CONVERSATION_ID,
                content,
                crate::now_iso()
            ],
        )
        .unwrap();
}
