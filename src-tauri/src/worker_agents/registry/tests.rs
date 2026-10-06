use super::*;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{definition_hash, read_meta};
use crate::worker_agents::test_support::*;
use rusqlite::{params, Connection};

const NOW: i64 = 1_000;

fn epoch(connection: &Connection) -> i64 {
    read_meta(connection).unwrap().registry_epoch
}

fn fts_count(connection: &Connection, revision_id: &str) -> i64 {
    connection
        .query_row(
            "SELECT COUNT(*) FROM worker_profile_fts WHERE revision_id = ?1",
            params![revision_id],
            |row| row.get(0),
        )
        .unwrap()
}

/// Creates source + tool + revision rows; returns (tool id, revision id).
fn catalog_tool(connection: &Connection, name: &str, effect: &str) -> (String, String) {
    connection
        .execute(
            "INSERT OR IGNORE INTO tool_selection_sources(id, kind, owner_principal, enabled)
             VALUES('src1', 'llang', 'owner', 1)",
            [],
        )
        .unwrap();
    let tool_id = format!("tool_{name}");
    let revision_id = format!("trev_{name}");
    connection
        .execute(
            "INSERT INTO tool_selection_catalog(id, source_id, backend_key, current_revision_id, enabled)
             VALUES(?1, 'src1', ?2, NULL, 1)",
            params![tool_id, format!("backend_{name}")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tool_selection_revisions(id, tool_id, schema_hash, description_hash,
                input_schema_json, output_schema_json, search_text, operations_json, objects_json,
                effect, backend_binding_json, created_at)
             VALUES(?1, ?2, ?3, ?3, '{}', NULL, 'x', '[]', '[]', ?4, '{}', 1)",
            params![revision_id, tool_id, "a".repeat(64), effect],
        )
        .unwrap();
    (tool_id, revision_id)
}

fn catalog_ref(tool_id: &str, revision_id: &str) -> ToolRef {
    ToolRef {
        kind: ToolRefKind::Catalog,
        key: tool_id.into(),
        catalog_revision_id: Some(revision_id.into()),
    }
}

fn saved(connection: &Connection, draft: &ProfileDraft) -> RevisionSummary {
    save_draft(connection, draft, "user_ipc", NOW).unwrap()
}

#[test]
fn migrate_twice_and_reseed_are_idempotent() {
    let connection = fresh_db();
    crate::worker_agents::schema::migrate(&connection).unwrap();
    crate::worker_agents::schema::migrate(&connection).unwrap();
    let drafts = [sample_draft("seed_agent", "seeded purpose")];
    seed_builtin(&connection, NOW, &drafts).unwrap();
    let first = list_agents(&connection).unwrap();
    let epoch_after_first = epoch(&connection);
    seed_builtin(&connection, NOW + 1, &drafts).unwrap();
    assert_eq!(list_agents(&connection).unwrap(), first);
    assert_eq!(epoch(&connection), epoch_after_first);
    let agent = &first[0];
    assert_eq!(agent.origin, "builtin");
    assert!(agent.enabled && agent.pinned_offer);
    let current = agent.current.as_ref().unwrap();
    assert_eq!(current.revision, 1);
    assert_eq!(current.review_state, ReviewState::Approved);
    assert_eq!(fts_count(&connection, &current.revision_id), 1);
    let created_by: String = connection
        .query_row(
            "SELECT created_by FROM worker_profile_revisions WHERE id = ?1",
            params![current.revision_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(created_by, "host_seed");
}

#[test]
fn seed_never_overwrites_user_edits() {
    let connection = fresh_db();
    let original = sample_draft("seed_agent", "original purpose");
    seed_builtin(&connection, NOW, std::slice::from_ref(&original)).unwrap();
    let mut edited = original.clone();
    edited.purpose = "user edited purpose".into();
    let draft = saved(&connection, &edited);
    approve_revision(
        &connection,
        "seed_agent",
        &draft.revision_id,
        &draft.definition_hash,
        NOW,
    )
    .unwrap();
    let before = list_agents(&connection).unwrap();
    seed_builtin(&connection, NOW + 5, &[original]).unwrap();
    let after = list_agents(&connection).unwrap();
    assert_eq!(before, after);
    assert_eq!(
        after[0].current.as_ref().unwrap().purpose,
        "user edited purpose"
    );
    assert_eq!(
        get_agent(&connection, "seed_agent")
            .unwrap()
            .revisions
            .len(),
        2
    );
}

#[test]
fn draft_is_not_current_not_indexed_and_keeps_the_epoch() {
    let connection = fresh_db();
    let before = epoch(&connection);
    let draft = saved(&connection, &sample_draft("alpha_agent", "alpha purpose"));
    assert_eq!(draft.review_state, ReviewState::Draft);
    assert_eq!(draft.revision, 1);
    assert_eq!(epoch(&connection), before);
    assert_eq!(fts_count(&connection, &draft.revision_id), 0);
    let agent = &list_agents(&connection).unwrap()[0];
    assert!(agent.current.is_none());
    assert_eq!(agent.draft.as_ref().unwrap().revision_id, draft.revision_id);
    assert_eq!(agent.origin, "user");
    assert!(agent.enabled && !agent.pinned_offer);
    // Identical definition is not a new revision.
    let again = saved(&connection, &sample_draft("alpha_agent", "alpha purpose"));
    assert_eq!(again.revision_id, draft.revision_id);
    // A changed one is revision 2.
    let second = saved(
        &connection,
        &sample_draft("alpha_agent", "alpha purpose v2"),
    );
    assert_eq!(second.revision, 2);
    assert_eq!(epoch(&connection), before);
}

#[test]
fn approve_rejects_a_hash_mismatch_and_a_tampered_row() {
    let connection = fresh_db();
    let draft = saved(&connection, &sample_draft("alpha_agent", "alpha purpose"));
    let wrong = definition_hash(&sample_draft("alpha_agent", "something else"));
    assert_eq!(
        approve_revision(&connection, "alpha_agent", &draft.revision_id, &wrong, NOW).unwrap_err(),
        "definition_hash_mismatch"
    );
    // Rows edited behind the registry's back no longer match the stored hash.
    connection
        .execute(
            "UPDATE worker_profile_revisions SET system_context = 'tampered' WHERE id = ?1",
            params![draft.revision_id],
        )
        .unwrap();
    assert_eq!(
        approve_revision(
            &connection,
            "alpha_agent",
            &draft.revision_id,
            &draft.definition_hash,
            NOW
        )
        .unwrap_err(),
        "definition_hash_mismatch"
    );
    let agent = get_summary(&connection, "alpha_agent").unwrap();
    assert!(agent.current.is_none());
    assert_eq!(epoch(&connection), 0);
    // Wrong profile for the revision.
    saved(&connection, &sample_draft("beta_agent", "beta"));
    assert!(approve_revision(
        &connection,
        "beta_agent",
        &draft.revision_id,
        &draft.definition_hash,
        NOW
    )
    .is_err());
}

#[test]
fn write_effect_catalog_tool_stays_draft() {
    let connection = fresh_db();
    let (tool, revision) = catalog_tool(&connection, "mutator", "write");
    let mut draft = sample_draft("writer_agent", "writes things");
    draft.tools.push(catalog_ref(&tool, &revision));
    let saved_draft = saved(&connection, &draft);
    assert_eq!(
        approve_revision(
            &connection,
            "writer_agent",
            &saved_draft.revision_id,
            &saved_draft.definition_hash,
            NOW
        )
        .unwrap_err(),
        "write_tools_not_supported"
    );
    let agent = get_summary(&connection, "writer_agent").unwrap();
    assert!(agent.current.is_none());
    assert_eq!(agent.draft.unwrap().review_state, ReviewState::Draft);
    assert_eq!(epoch(&connection), 0);
}

#[test]
fn read_catalog_tool_resolves_its_effect_from_the_revision_row() {
    let connection = fresh_db();
    let (tool, revision) = catalog_tool(&connection, "reader", "read");
    let mut draft = sample_draft("reader_agent", "reads things");
    draft.tools.push(catalog_ref(&tool, &revision));
    let saved_draft = saved(&connection, &draft);
    let effects: Vec<String> = connection
        .prepare("SELECT effect FROM worker_profile_tools WHERE profile_revision_id = ?1 ORDER BY ordinal")
        .unwrap()
        .query_map(params![saved_draft.revision_id], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(effects, ["read", "read"]);
    approve_revision(
        &connection,
        "reader_agent",
        &saved_draft.revision_id,
        &saved_draft.definition_hash,
        NOW,
    )
    .unwrap();
    // A recorded effect that is not read-only blocks approval even if the live row says read.
    let other = saved(&connection, &{
        let mut next = draft.clone();
        next.purpose = "reads things again".into();
        next
    });
    connection
        .execute(
            "UPDATE worker_profile_tools SET effect = 'write' WHERE profile_revision_id = ?1 AND tool_kind = 'catalog'",
            params![other.revision_id],
        )
        .unwrap();
    assert_eq!(
        approve_revision(
            &connection,
            "reader_agent",
            &other.revision_id,
            &other.definition_hash,
            NOW
        )
        .unwrap_err(),
        "write_tools_not_supported"
    );
    // An unavailable or unknown catalog tool is rejected at save time.
    let mut bad = sample_draft("reader_agent", "bad");
    bad.tools.push(catalog_ref("tool_missing", &revision));
    assert!(save_draft(&connection, &bad, "user_ipc", NOW).is_err());
    let mut missing_pin = sample_draft("reader_agent", "bad");
    missing_pin.tools.push(ToolRef {
        kind: ToolRefKind::Catalog,
        key: tool.clone(),
        catalog_revision_id: None,
    });
    assert!(save_draft(&connection, &missing_pin, "user_ipc", NOW).is_err());
    connection
        .execute(
            "UPDATE tool_selection_catalog SET enabled = 0 WHERE id = ?1",
            params![tool],
        )
        .unwrap();
    assert!(save_draft(&connection, &draft, "user_ipc", NOW + 1).is_err());
}

#[test]
fn approve_supersedes_the_previous_current_and_bumps_the_epoch() {
    let connection = fresh_db();
    let first = saved(&connection, &sample_draft("alpha_agent", "first purpose"));
    let summary = approve_revision(
        &connection,
        "alpha_agent",
        &first.revision_id,
        &first.definition_hash,
        NOW,
    )
    .unwrap();
    assert_eq!(epoch(&connection), 1);
    assert_eq!(
        summary.current.as_ref().unwrap().review_state,
        ReviewState::Approved
    );
    assert_eq!(fts_count(&connection, &first.revision_id), 1);

    let second = saved(&connection, &sample_draft("alpha_agent", "second purpose"));
    assert_eq!(epoch(&connection), 1);
    assert_eq!(fts_count(&connection, &second.revision_id), 0);
    let summary = approve_revision(
        &connection,
        "alpha_agent",
        &second.revision_id,
        &second.definition_hash,
        NOW + 1,
    )
    .unwrap();
    assert_eq!(epoch(&connection), 2);
    assert_eq!(
        summary.current.as_ref().unwrap().revision_id,
        second.revision_id
    );
    assert!(summary.draft.is_none());
    assert_eq!(fts_count(&connection, &first.revision_id), 0);
    assert_eq!(fts_count(&connection, &second.revision_id), 1);
    let detail = get_agent(&connection, "alpha_agent").unwrap();
    let state_of = |id: &str| {
        detail
            .revisions
            .iter()
            .find(|r| r.revision_id == id)
            .unwrap()
            .review_state
    };
    assert_eq!(state_of(&first.revision_id), ReviewState::Superseded);
    assert_eq!(state_of(&second.revision_id), ReviewState::Approved);
    // An approved revision cannot be approved again.
    assert_eq!(
        approve_revision(
            &connection,
            "alpha_agent",
            &second.revision_id,
            &second.definition_hash,
            NOW
        )
        .unwrap_err(),
        "revision_not_draft"
    );
}

#[test]
fn approving_a_newer_draft_supersedes_older_drafts() {
    let connection = fresh_db();
    let older = saved(&connection, &sample_draft("alpha_agent", "older"));
    let newer = saved(&connection, &sample_draft("alpha_agent", "newer"));
    approve_revision(
        &connection,
        "alpha_agent",
        &newer.revision_id,
        &newer.definition_hash,
        NOW,
    )
    .unwrap();
    assert_eq!(
        approve_revision(
            &connection,
            "alpha_agent",
            &older.revision_id,
            &older.definition_hash,
            NOW
        )
        .unwrap_err(),
        "revision_not_draft"
    );
}

#[test]
fn set_enabled_bumps_the_epoch_and_mode_does_not() {
    let connection = fresh_db();
    saved(&connection, &sample_draft("alpha_agent", "alpha"));
    let summary = set_enabled(&connection, "alpha_agent", false, NOW).unwrap();
    assert!(!summary.enabled);
    assert_eq!(epoch(&connection), 1);
    assert!(set_enabled(&connection, "missing_agent", true, NOW).is_err());
    assert_eq!(epoch(&connection), 1);
    assert_eq!(
        get_web_search_mode(&connection).unwrap(),
        WebSearchMode::Inline
    );
    set_web_search_mode(&connection, WebSearchMode::Worker).unwrap();
    assert_eq!(
        get_web_search_mode(&connection).unwrap(),
        WebSearchMode::Worker
    );
    assert_eq!(epoch(&connection), 1);
}

#[test]
fn skills_are_immutable_and_deduplicated() {
    let connection = fresh_db();
    let epoch_before = epoch(&connection);
    let first = save_skill(
        &connection,
        &SkillDraft {
            skill_id: None,
            name: "style".into(),
            body: "be brief".into(),
        },
        NOW,
    )
    .unwrap();
    assert!(first.skill_id.starts_with("wskill_"));
    let same = save_skill(
        &connection,
        &SkillDraft {
            skill_id: Some(first.skill_id.clone()),
            name: "style".into(),
            body: "be brief".into(),
        },
        NOW + 1,
    )
    .unwrap();
    assert_eq!(same, first);
    let changed = save_skill(
        &connection,
        &SkillDraft {
            skill_id: Some(first.skill_id.clone()),
            name: "style".into(),
            body: "be thorough".into(),
        },
        NOW + 2,
    )
    .unwrap();
    assert_eq!(changed.skill_id, first.skill_id);
    assert_ne!(changed.revision_id, first.revision_id);
    let body: String = connection
        .query_row(
            "SELECT body FROM worker_skill_revisions WHERE id = ?1",
            params![first.revision_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, "be brief");
    assert_eq!(
        save_skill(
            &connection,
            &SkillDraft {
                skill_id: Some(first.skill_id.clone()),
                name: "renamed".into(),
                body: "x".into()
            },
            NOW
        )
        .unwrap_err(),
        "skill_name_immutable"
    );
    assert!(save_skill(
        &connection,
        &SkillDraft {
            skill_id: Some("wskill_missing".into()),
            name: "style".into(),
            body: "x".into()
        },
        NOW
    )
    .is_err());
    assert!(save_skill(
        &connection,
        &SkillDraft {
            skill_id: None,
            name: "n".into(),
            body: String::new()
        },
        NOW
    )
    .is_err());
    assert_eq!(epoch(&connection), epoch_before);

    // A profile can link the revision, and loading shows it.
    let mut draft = sample_draft("skilled_agent", "uses a skill");
    draft.skill_revision_ids = vec![changed.revision_id.clone()];
    let saved_draft = saved(&connection, &draft);
    approve_revision(
        &connection,
        "skilled_agent",
        &saved_draft.revision_id,
        &saved_draft.definition_hash,
        NOW,
    )
    .unwrap();
    let loaded =
        crate::worker_agents::loader::load_revision(&connection, &saved_draft.revision_id).unwrap();
    assert_eq!(loaded.skills[0].body, "be thorough");
    let mut unknown = sample_draft("skilled_agent", "unknown skill");
    unknown.skill_revision_ids = vec!["wskillrev_missing".into()];
    assert!(save_draft(&connection, &unknown, "user_ipc", NOW).is_err());
}

#[test]
fn save_draft_validation_failures() {
    let connection = fresh_db();
    let fails = |mutate: &dyn Fn(&mut ProfileDraft)| {
        let mut draft = sample_draft("valid_agent", "valid purpose");
        mutate(&mut draft);
        save_draft(&connection, &draft, "user_ipc", NOW).is_err()
    };
    assert!(fails(&|d| d.profile_id = "Bad-Id".into()));
    assert!(fails(&|d| d.profile_id = "ab".into()));
    assert!(fails(&|d| d.profile_id = "1abc".into()));
    assert!(fails(&|d| d.purpose = String::new()));
    assert!(fails(&|d| d.purpose = "x".repeat(2001)));
    assert!(fails(&|d| d.system_context = String::new()));
    assert!(fails(&|d| d.system_context = "x".repeat(16_385)));
    assert!(fails(&|d| d.tools.clear()));
    assert!(fails(&|d| d.tools = (0..9)
        .map(|i| ToolRef {
            kind: ToolRefKind::Catalog,
            key: format!("t{i}"),
            catalog_revision_id: Some("r".into()),
        })
        .collect()));
    assert!(fails(&|d| d.tools.push(d.tools[0].clone())));
    assert!(fails(&|d| d.tools[0].key = "shell_exec".into()));
    assert!(fails(
        &|d| d.input_schema = serde_json::json!({"type": "nonsense"})
    ));
    assert!(fails(&|d| d.input_schema = serde_json::json!("string")));
    assert!(fails(&|d| d.output_schema = None));
    assert!(fails(
        &|d| d.output_schema = Some(serde_json::json!({"type": 5}))
    ));
    assert!(fails(&|d| {
        d.output_kind = OutputKind::WebClaimsV1;
        d.output_schema = None;
        d.completion.sources_must_be_host_recorded = false;
    }));
    assert!(fails(&|d| d.limits.max_steps = 0));
    assert!(fails(&|d| d.limits.max_steps = 21));
    assert!(fails(&|d| d.limits.deadline_ms = 999));
    assert!(fails(&|d| d.limits.deadline_ms = 300_001));
    assert!(fails(&|d| {
        d.limits.deadline_ms = 10_000;
        d.limits.sync_wait_ms = 10_001;
    }));
    assert!(fails(
        &|d| d.input_schema = serde_json::json!({"$ref": "http://127.0.0.1:9/schema.json"})
    ));
    assert!(save_draft(
        &connection,
        &sample_draft("valid_agent", "ok"),
        "other",
        NOW
    )
    .is_err());
    // A valid web-claims profile is accepted.
    let mut claims = sample_draft("claims_agent", "web claims");
    claims.output_kind = OutputKind::WebClaimsV1;
    claims.output_schema = None;
    claims.completion = CompletionCriteria {
        min_items: 1,
        sources_must_be_host_recorded: true,
    };
    assert!(save_draft(&connection, &claims, "user_ipc", NOW).is_ok());
    // Nothing from the failed attempts was persisted.
    assert_eq!(list_agents(&connection).unwrap().len(), 1);
}

#[test]
fn blocklist_lists_pages_removes_and_never_expires() {
    let connection = fresh_db();
    for (index, created) in [(1, 100), (2, 300), (3, 300), (4, 200)] {
        connection
            .execute(
                "INSERT INTO worker_url_blocklist(url_hash, host, reason, created_at_ms) VALUES(?1,?2,'injection',?3)",
                params![format!("{index:064}"), format!("h{index}.example"), created],
            )
            .unwrap();
    }
    let all = list_blocklist(&connection, 50, None).unwrap();
    let order: Vec<_> = all.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(
        order,
        ["h2.example", "h3.example", "h4.example", "h1.example"]
    );
    let page = list_blocklist(&connection, 2, None).unwrap();
    assert_eq!(page.len(), 2);
    let next = list_blocklist(&connection, 2, Some(&page[1].url_hash)).unwrap();
    let next_hosts: Vec<_> = next.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(next_hosts, ["h4.example", "h1.example"]);
    assert!(list_blocklist(&connection, 2, Some(&"f".repeat(64)))
        .unwrap()
        .is_empty());
    // Seeding, approval and epoch changes never prune entries; only remove deletes.
    seed_builtin(
        &connection,
        i64::MAX / 2,
        &[sample_draft("seed_agent", "p")],
    )
    .unwrap();
    assert_eq!(list_blocklist(&connection, 50, None).unwrap().len(), 4);
    assert!(remove_blocklist(&connection, &format!("{:064}", 1)).unwrap());
    assert!(!remove_blocklist(&connection, &format!("{:064}", 1)).unwrap());
    assert_eq!(list_blocklist(&connection, 50, None).unwrap().len(), 3);
}

#[test]
fn task_listing_has_no_bodies_and_is_limited() {
    let connection = fresh_db();
    insert_user_message(&connection, "msg1", "hello");
    let revision = insert_revision(
        &connection,
        &sample_draft("alpha_agent", "alpha"),
        ReviewState::Approved,
        true,
    );
    for index in 0..3 {
        connection
            .execute(
                "INSERT INTO worker_tasks(id, conversation_id, input_message_id, origin_job_key, profile_id,
                    profile_revision_id, idempotency_key, input_json, state, delivery, deadline_at_ms,
                    sync_wait_until_ms, result_json, failure_code, created_at_ms, updated_at_ms)
                 VALUES(?1, ?2, 'msg1', 'job', 'alpha_agent', ?3, ?4, '{\"q\":\"secret\"}', 'failed',
                    'suppressed', 10, 10, '{\"r\":1}', 'no_results', ?5, ?5)",
                params![
                    format!("wtask_{index}"),
                    crate::PRIMARY_CONVERSATION_ID,
                    revision,
                    format!("key{index}"),
                    index
                ],
            )
            .unwrap();
    }
    let tasks = list_tasks(&connection, 2).unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task_id, "wtask_2");
    assert_eq!(tasks[0].failure_code.as_deref(), Some("no_results"));
    assert_eq!(list_tasks(&connection, 0).unwrap().len(), 1);
    assert_eq!(list_tasks(&connection, 10_000).unwrap().len(), 3);
}

#[test]
fn embeddings_are_stored_as_little_endian_f32() {
    let connection = fresh_db();
    let revision = insert_revision(
        &connection,
        &sample_draft("alpha_agent", "alpha"),
        ReviewState::Approved,
        true,
    );
    index_embedding(&connection, &revision, "model1", &[1.0, -2.5]).unwrap();
    index_embedding(&connection, &revision, "model1", &[0.5, 0.25, 0.125]).unwrap();
    let (dimension, vector): (i64, Vec<u8>) = connection
        .query_row(
            "SELECT dimension, vector FROM worker_profile_embeddings WHERE revision_id = ?1 AND model_hash = 'model1'",
            params![revision],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(dimension, 3);
    assert_eq!(&vector[..4], &0.5f32.to_le_bytes());
    assert!(index_embedding(&connection, "missing", "model1", &[1.0]).is_err());
    assert!(index_embedding(&connection, &revision, "model1", &[]).is_err());
    assert!(index_embedding(&connection, &revision, "model1", &[f32::NAN]).is_err());
}
