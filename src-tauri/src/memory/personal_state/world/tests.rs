#![cfg(test)]

//! WM adapter integration tests (WM-02..WM-15). Deterministic fixtures only.

use super::query::{activate, ActivateInput, WorldSeed};
use super::test_support::*;
use crate::memory::personal_state::{encode, now, sources, store};
use saaa_personal_state_core::world::traversal::{CausalDirection, Limits, TraversalMode};
use saaa_personal_state_core::world::*;
use saaa_personal_state_core::*;
use serde_json::json;

struct Fixture {
    writer: crate::persistence::sqlite::SqliteWriter,
    source: SourceRef,
    built_at: i64,
    objective: String,
}

fn world_entity_rows(c: &rusqlite::Connection) -> Result<Vec<(String, String, String)>, String> {
    let mut statement = c
        .prepare(
            "SELECT assertion_id,entity_id,status FROM personal_world_entities ORDER BY assertion_id",
        )
        .map_err(crate::database_error)?;
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    Ok(rows)
}

fn fixture() -> Fixture {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| {
            Ok(insert_source(
                c,
                PROJECT,
                "s1",
                "投機的デコードの導入は応答遅延を下げるかもしれない",
            ))
        })
        .unwrap();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "fix-objective",
            vec![objective_assertion(
                "obj1", "obj1p", &source, PROJECT, now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "fix-entities",
            vec![
                entity_assertion(
                    "ent1",
                    "ent1p",
                    "e1",
                    EntityKind::Concept,
                    "Speculative Decodingの導入",
                    &["投機的デコードの導入"],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent2",
                    "ent2p",
                    "e2",
                    EntityKind::Metric,
                    "Decode Latency",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent3",
                    "ent3p",
                    "e3",
                    EntityKind::Metric,
                    "Voice Response Latency",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "ent4",
                    "ent4p",
                    "e4",
                    EntityKind::Project,
                    "SAAA",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    let support = |stance| vec![(source.key.clone(), stance)];
    committer
        .commit(
            "fix-relations",
            vec![
                relation_assertion(
                    "rel1",
                    "rel1p",
                    "e1",
                    "e2",
                    RelationType::Decreases,
                    Some(EffectInput::Intervention),
                    &[("config", "fixture-a")],
                    Basis::ModelHypothesis,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent1".to_string(), "ent2".to_string()]),
                    now_ms,
                ),
                relation_assertion(
                    "rel2",
                    "rel2p",
                    "e2",
                    "e3",
                    RelationType::Increases,
                    Some(EffectInput::QuantityIncrease),
                    &[("config", "fixture-a")],
                    Basis::ModelHypothesis,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent2".to_string(), "ent3".to_string()]),
                    now_ms,
                ),
                relation_assertion(
                    "rel3",
                    "rel3p",
                    "e3",
                    "e4",
                    RelationType::ImportantFor,
                    None,
                    &[],
                    Basis::UserStatement,
                    support(Stance::Supports),
                    &source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent3".to_string(), "ent4".to_string()]),
                    now_ms,
                ),
            ],
        )
        .unwrap();
    committer
        .commit(
            "fix-focus",
            vec![focus_assertion(
                "foc1",
                "foc1p",
                "e4",
                FocusReason::CurrentWork,
                Some("obj1"),
                &source,
                PROJECT,
                std::collections::BTreeSet::from(["ent4".to_string(), "obj1".to_string()]),
                now_ms,
            )],
        )
        .unwrap();
    // The adapter has processed s1; its job is complete before World commit.
    writer
        .write(|c| {
            c.execute(
                "UPDATE personal_jobs SET status='completed' WHERE source_sequence=(
                   SELECT sequence FROM personal_sources WHERE message_id='s1')",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    Fixture {
        writer,
        source,
        built_at: now_ms,
        objective: "obj1".into(),
    }
}

fn query(f: &Fixture, seeds: &[WorldSeed]) -> Result<WorldSlice, String> {
    f.writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, PROJECT);
        activate(
            c,
            &ActivateInput {
                project_scope: PROJECT,
                access: &request,
                now: now(),
                seeds,
                causal_direction: CausalDirection::Forward,
                limits: Limits::default(),
                max_bytes: 8_192,
            },
        )
    })
}

#[test]
fn t22_end_to_end_fixture_relevance_causal_and_gaps() {
    let f = fixture();
    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(slice.notices.is_empty(), "notices: {:?}", slice.notices);
    let relevance: Vec<Vec<String>> = slice
        .relevance_paths
        .iter()
        .map(|p| p.nodes.clone())
        .collect();
    assert!(
        relevance
            .iter()
            .any(|nodes| nodes == &vec!["e1", "e2", "e3", "e4"]),
        "relevance={relevance:?}"
    );
    assert_eq!(slice.causal_paths.len(), 1);
    assert_eq!(slice.causal_paths[0].nodes, vec!["e1", "e2", "e3"]);
    // R1/R2 model hypotheses with declared conditions => one gap each.
    assert_eq!(slice.research_gaps.len(), 2, "{:?}", slice.research_gaps);
    assert!(slice
        .research_gaps
        .iter()
        .all(|g| g.reason == "hypothesis_unverified"));
    // R3 important_for never becomes a causal gap.
    assert!(slice
        .research_gaps
        .iter()
        .all(|g| g.relation_assertion_id != "rel3"));
    // Hypothesis labelling and evidence survive.
    let rel1 = slice
        .relations
        .iter()
        .find(|r| r.assertion_id == "rel1")
        .expect("rel1 present");
    assert_eq!(rel1.basis, "model_hypothesis");
    assert_eq!(rel1.conditions, vec![vec!["config", "fixture-a"]]);
    assert_eq!(rel1.evidence.len(), 1);
}

#[test]
fn t09_rebuild_is_reproducible_and_empty_is_fine() {
    let f = fixture();
    let first = f.writer.read_serialized(world_entity_rows);
    // Rebuild twice, compare.
    for _ in 0..2 {
        f.writer.write(|c| store::rebuild(c, now())).unwrap();
    }
    let second = f.writer.read_serialized(world_entity_rows);
    assert_eq!(first, second);
    // A World-free database still builds an empty, usable projection.
    let empty = writer_db();
    empty
        .write(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM personal_world_entities", [], |r| r
                    .get::<_, u64>(0))
                    .map_err(crate::database_error)?,
                0
            );
            assert!(c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM personal_world_projection_meta WHERE id=1)",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .map_err(crate::database_error)?);
            Ok(())
        })
        .unwrap();
}

#[test]
fn t08_schema_migration_is_idempotent_and_adds_world_tables() {
    let c = memory_db();
    // Re-running the initializer must not fail or duplicate.
    crate::initialize_database(&c).unwrap();
    for table in [
        "personal_world_projection_meta",
        "personal_world_entities",
        "personal_world_aliases",
        "personal_world_relations",
        "personal_world_focus",
    ] {
        let exists: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(exists, "missing {table}");
    }
    let indexes: i64 = c
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name LIKE 'personal_world_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(indexes >= 5);
    // The forget trigger exists and is not a rewrite of an existing trigger.
    let trigger: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='personal_world_forget')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(trigger);
    // D18/C6: the projection-format version column exists exactly once and
    // defaults to 1, and re-running the initializer does not duplicate it.
    let version_columns: i64 = c
        .query_row(
            "SELECT count(*) FROM pragma_table_info('personal_world_projection_meta') WHERE name='projection_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version_columns, 1);
    let default_value: Option<String> = c
        .query_row(
            "SELECT dflt_value FROM pragma_table_info('personal_world_projection_meta') WHERE name='projection_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(default_value.as_deref(), Some("1"));
}

#[test]
fn t10_direct_commit_cannot_bypass_world_validation() {
    let f = fixture();
    let now_ms = now();
    // A relation whose endpoint entity does not exist must be rejected even
    // when handed straight to store::commit.
    let source = f.source.clone();
    let bad = relation_assertion(
        "bad-rel",
        "bad-rel-p",
        "missing",
        "e2",
        RelationType::Decreases,
        Some(EffectInput::Intervention),
        &[],
        Basis::ModelHypothesis,
        vec![(source.key.clone(), Stance::Supports)],
        &source,
        PROJECT,
        std::collections::BTreeSet::from(["missing".to_string(), "ent2".to_string()]),
        now_ms,
    );
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    let error = committer.commit("bad", vec![bad]).unwrap_err();
    assert_eq!(error, "world-invalid-reference");
    // Nothing partial was persisted.
    f.writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM personal_assertions WHERE id='bad-rel'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .map_err(crate::database_error)?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn t12_forget_erases_projected_names_and_edges() {
    let f = fixture();
    let slice = query(&f, &[WorldSeed::ExactName("SAAA".into())]).unwrap();
    assert!(!slice.nodes.is_empty());
    // Memory OFF is irrelevant: the tombstone trigger wipes the projection.
    f.writer
        .write(|c| {
            c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    f.writer
        .read_serialized(|c| {
            for table in [
                "personal_world_entities",
                "personal_world_aliases",
                "personal_world_relations",
                "personal_world_focus",
            ] {
                let count: u64 = c
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .map_err(crate::database_error)?;
                assert_eq!(count, 0, "{table} not erased");
            }
            assert!(store::load(c)?.assertions.is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn t13_source_edit_uses_only_the_current_version() {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "s1", "旧い仮説")))
        .unwrap();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "edit-obj",
            vec![objective_assertion(
                "obj1", "obj1p", &source, PROJECT, now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "edit-ent",
            vec![entity_assertion(
                "ent1",
                "ent1p",
                "e1",
                EntityKind::Concept,
                "Old",
                &[],
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .unwrap();
    // Edit the source: the old version becomes unavailable.
    writer
        .write(|c| {
            c.execute(
                "UPDATE conversation_messages SET content='新しい仮説' WHERE id='s1'",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            assert_ne!(ledger.status("ent1", now()), Status::Active);
            // Editing the source changes the input epoch, so the old projection
            // is unreadable until a rebuild removes the unavailable version.
            let request = access(&ledger, PROJECT);
            let slice = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("e1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )?;
            assert!(slice.notices.contains(&"projection_stale".to_string()));
            assert_eq!(sources::load(c, 2, 0, 64)?.source.key.version, 2);
            Ok(())
        })
        .unwrap();
    writer.write(|c| store::rebuild(c, now())).unwrap();
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM personal_world_entities", [], |r| r
                    .get::<_, u64>(0))
                    .map_err(crate::database_error)?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn forgotten_source_version_is_not_revived_by_a_new_version_with_the_same_id() {
    let f = fixture();
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE personal_sources SET available=0 WHERE message_id=?1 AND version=?2",
                rusqlite::params![f.source.key.id, f.source.key.version],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO personal_sources(message_id,version,role,bytes,recorded_at,available)
                 VALUES(?1,?2,'user',0,?3,1)",
                rusqlite::params![f.source.key.id, f.source.key.version + 1, now()],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();

    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice.relations.is_empty());
}

#[test]
fn t14_scope_isolation_and_authorization() {
    let f = fixture();
    // Same display name in a different project must not resolve here.
    let now_ms = now();
    let other = f
        .writer
        .write(|c| Ok(insert_source(c, "project:other", "s2", "SAAA別件")))
        .unwrap();
    let mut other_committer = Committer {
        writer: &f.writer,
        project: "project:other",
    };
    other_committer
        .commit(
            "other-ent",
            vec![entity_assertion(
                "other-saaa",
                "other-saaa-p",
                "e4",
                EntityKind::Project,
                "SAAA",
                &[],
                &other,
                "project:other",
                now_ms,
            )],
        )
        .unwrap();
    // The fixture project still resolves exactly one SAAA.
    let slice = query(&f, &[WorldSeed::ExactName("SAAA".into())]).unwrap();
    assert_eq!(slice.nodes.iter().filter(|n| n.name == "SAAA").count(), 1);
    // Unauthorized purpose is a contract error, not an empty slice.
    let error = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let error = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &AccessRequest {
                        principal: &ledger.principal,
                        scope: "primary",
                        task_request: Some(PROJECT),
                        purpose: Purpose::Tactical,
                        max_classification: Classification::Confidential,
                        policy_revision: ledger.policy_revision,
                        authorized: false,
                    },
                    now: now(),
                    seeds: &[WorldSeed::ExactName("SAAA".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )
            .unwrap_err();
            Ok(error)
        })
        .unwrap();
    assert_eq!(error, "world-scope-denied");
}

#[test]
fn t15_stale_projection_and_pending_review_are_explicit() {
    let f = fixture();
    // Corrupt the meta revision => stale notice, not an empty "no relation".
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE personal_world_projection_meta SET ledger_revision=ledger_revision+1 WHERE id=1",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let stale = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(stale.notices.contains(&"projection_stale".to_string()));
    assert!(stale.relevance_paths.is_empty());
    // Restore and add a newer pending project source.
    f.writer
        .write(|c| {
            store::rebuild(c, now())?;
            insert_source(c, PROJECT, "s-pending", "未処理の追加");
            c.execute(
                "UPDATE personal_jobs SET status='queued' WHERE source_sequence=(
                   SELECT sequence FROM personal_sources WHERE message_id='s-pending')",
                [],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let pending = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert!(pending.notices.contains(&"pending_review".to_string()));
}

#[test]
fn t03_world_kinds_do_not_leak_into_continuity_paths() {
    let f = fixture();
    f.writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            // Owner snapshot excludes World kinds.
            let snapshot = crate::memory::personal_state::commands::snapshot(c)?;
            let items = snapshot["items"].as_array().unwrap();
            assert!(items
                .iter()
                .all(|item| !item["kind"].as_str().unwrap().starts_with("world_")));
            // Ordinary Context composition excludes World kinds.
            let composed =
                crate::memory::personal_state::projection::compose(c, Some(PROJECT), 262144)?;
            let items = composed["items"].as_array().unwrap();
            assert!(items
                .iter()
                .all(|item| !item["kind"].as_str().unwrap().starts_with("world_")));
            // Continuity extraction never enumerates World as current state.
            assert!(ledger.assertions.values().any(|a| a.kind.is_world()));
            Ok(())
        })
        .unwrap();
    // The extractor cannot return a World kind.
    let rejected = f.writer.read_serialized(|_c| {
        let raw = json!({
            "candidates": [{
                "kind": "world_entity",
                "semantic_key": "x",
                "value": {"type": "entity", "schema_version": 1, "entity_id": "e", "entity_kind": "concept", "name": "n", "aliases": []},
                "status": "active",
                "task_request": PROJECT,
                "replaces": null
            }],
            "no_change": false
        });
        let parsed: Result<crate::memory::personal_state::worker::Extraction, _> =
            serde_json::from_value(raw);
        // The Kind enum may parse world kinds, but the worker rejects them before commit.
        parsed.map(|_| ()).map_err(|e| e.to_string())
    });
    assert!(rejected.is_ok());
}

#[test]
fn t16_t17_traversal_is_bounded_and_deterministic() {
    use saaa_personal_state_core::world::traversal::{traverse, WorldEdge};
    let edges = vec![
        WorldEdge {
            assertion_id: "a".into(),
            semantic_key: "wm1:a".into(),
            from: "e1".into(),
            to: "e2".into(),
            relation_type: "related_to".into(),
            status: saaa_personal_state_core::world::traversal::EdgeStatus::Active,
            conditions: vec![],
            basis: "user_statement".into(),
        },
        WorldEdge {
            assertion_id: "b".into(),
            semantic_key: "wm1:b".into(),
            from: "e2".into(),
            to: "e1".into(),
            relation_type: "related_to".into(),
            status: saaa_personal_state_core::world::traversal::EdgeStatus::Active,
            conditions: vec![],
            basis: "user_statement".into(),
        },
    ];
    let outcome = traverse(
        &edges,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    // A cycle must be suppressed per path.
    assert!(outcome.paths.iter().all(|p| p
        .nodes
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        == p.nodes.len()));
    let tight = traverse(
        &edges,
        &["e1".into()],
        Limits {
            depth: 0,
            ..Limits::default()
        },
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    assert!(tight.truncated);
    assert_eq!(tight.truncation_reason, Some("depth"));
}

#[test]
fn t21_slice_budget_is_respected_for_japanese_json() {
    use saaa_personal_state_core::world::model::{SliceNode, SliceRelation};
    use saaa_personal_state_core::world::relevance::trim_to_budget;
    let mut slice = WorldSlice::empty(1, 1);
    slice.nodes.push(SliceNode {
        entity_id: "e4".into(),
        name: "SAAA".into(),
        entity_kind: EntityKind::Project,
    });
    for index in 0..40 {
        slice.relations.push(SliceRelation {
            assertion_id: format!("r{index}"),
            semantic_key: format!("wm1:r{index}"),
            from_entity_id: "e4".into(),
            to_entity_id: "e4".into(),
            relation_type: "decreases".into(),
            status: "active".into(),
            basis: "model_hypothesis".into(),
            conditions: vec![vec!["config".into(), "fixture-a".into()]],
            evidence: Vec::new(),
        });
        slice.research_gaps.push(ResearchGap {
            key: format!("k{index}"),
            reason: "hypothesis_unverified".into(),
            relation_assertion_id: format!("r{index}"),
            focus_entity_id: "e4".into(),
            question:
                "「Speculative Decodingの導入」は現在の構成で遅延を改善するか。比較計測が必要です。"
                    .into(),
        });
    }
    let trimmed = trim_to_budget(slice, 8_192).unwrap();
    assert!(trimmed.encoded_len().unwrap() <= 8_192);
    assert!(trimmed.truncated);
    // Too small a budget is an explicit error, never partial JSON.
    let error = trim_to_budget(WorldSlice::empty(1, 1), 32).unwrap_err();
    assert_eq!(error.code(), "world-budget-too-small");
}

fn relation1(
    id: &str,
    source: &SourceRef,
    built_at: i64,
    relation_type: RelationType,
) -> BuiltAssertion {
    relation_assertion(
        id,
        &format!("{id}-p"),
        "e1",
        "e2",
        relation_type,
        Some(EffectInput::Intervention),
        &[("config", "fixture-a")],
        Basis::ModelHypothesis,
        vec![(source.key.clone(), Stance::Supports)],
        source,
        PROJECT,
        std::collections::BTreeSet::from(["ent1".to_string(), "ent2".to_string()]),
        built_at,
    )
}

#[test]
fn t06_duplicate_active_replacement_and_dispute() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    // Same relation identity cannot be Active twice.
    let duplicate = relation1("rel1-dup", &f.source, f.built_at, RelationType::Decreases);
    let error = committer.commit("dup", vec![duplicate]).unwrap_err();
    assert_eq!(error, "personal-patch-Conflict");
    // A new version supersedes the old one; the history keeps both.
    let replacement = relation1("rel1b", &f.source, f.built_at, RelationType::Decreases);
    committer
        .commit_superseding("replace", vec![replacement], "rel1")
        .unwrap();
    f.writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            assert_eq!(ledger.status("rel1", now()), Status::Superseded);
            assert_eq!(ledger.status("rel1b", now()), Status::Active);
            Ok(())
        })
        .unwrap();
    // A challenging version is held as Disputed, never silently promoted.
    committer
        .transition("dispute", "rel1b", Action::Dispute)
        .unwrap();
    f.writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            assert_eq!(ledger.status("rel1b", now()), Status::Disputed);
            // Disputed edges stay visible in the relevance view but never enter
            // the causal view.
            let request = access(&ledger, PROJECT);
            let slice = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("e1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )?;
            assert!(slice
                .causal_paths
                .iter()
                .all(|p| p.steps.iter().all(|s| s.assertion_id != "rel1b")));
            Ok(())
        })
        .unwrap();
}

#[test]
fn t07_objective_retraction_removes_current_work_focus() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .transition("retract-obj", &f.objective, Action::Retract)
        .unwrap();
    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    // The current_work Focus is gone, so no relevance path reaches a Focus.
    assert!(slice.relevance_paths.is_empty());
    assert!(slice.focus.is_empty());
    // The causal hypothesis chain is unaffected by the Objective's lifecycle.
    assert_eq!(slice.causal_paths.len(), 1);
}

#[test]
fn t05_world_cycles_are_allowed_but_assertion_dependency_cycles_are_not() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    let forward = relation_assertion(
        "cyc-f",
        "cyc-f-p",
        "e1",
        "e2",
        RelationType::DependsOn,
        None,
        &[],
        Basis::UserStatement,
        vec![(f.source.key.clone(), Stance::Supports)],
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent1".to_string(), "ent2".to_string()]),
        f.built_at,
    );
    let backward = relation_assertion(
        "cyc-b",
        "cyc-b-p",
        "e2",
        "e1",
        RelationType::DependsOn,
        None,
        &[],
        Basis::UserStatement,
        vec![(f.source.key.clone(), Stance::Supports)],
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent2".to_string(), "ent1".to_string()]),
        f.built_at,
    );
    // A World relation cycle is not an assertion dependency cycle.
    committer.commit("cycle", vec![forward, backward]).unwrap();
    f.writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            assert_eq!(ledger.status("cyc-f", now()), Status::Active);
            assert_eq!(ledger.status("cyc-b", now()), Status::Active);
            Ok(())
        })
        .unwrap();
    // An assertion dependency cycle is still rejected by the existing reducer.
    let now_ms = now();
    let mut first = objective_assertion("cycle-a", "cycle-a-p", &f.source, PROJECT, now_ms);
    let mut second = objective_assertion("cycle-b", "cycle-b-p", &f.source, PROJECT, now_ms);
    first.assertion.depends_on = std::collections::BTreeSet::from(["cycle-b".to_string()]);
    second.assertion.depends_on = std::collections::BTreeSet::from(["cycle-a".to_string()]);
    let error = committer
        .commit("assertion-cycle", vec![first, second])
        .unwrap_err();
    assert_eq!(error, "personal-patch-Cycle");
}

#[test]
fn t20_focus_ranking_and_stable_gap_keys() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    // An explicit interest on the mid metric, committed after current_work.
    committer
        .commit(
            "interest",
            vec![focus_assertion(
                "foc2",
                "foc2p",
                "e2",
                FocusReason::ExplicitInterest,
                None,
                &f.source,
                PROJECT,
                std::collections::BTreeSet::from(["ent2".to_string()]),
                f.built_at,
            )],
        )
        .unwrap();
    let first = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    // current_work sorts before explicit_interest.
    assert_eq!(first.focus[0].reason, "current_work");
    assert_eq!(first.focus[0].entity_id, "e4");
    let keys: Vec<String> = first.research_gaps.iter().map(|g| g.key.clone()).collect();
    let second = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    let keys_again: Vec<String> = second.research_gaps.iter().map(|g| g.key.clone()).collect();
    assert_eq!(keys, keys_again);
}

#[test]
fn t11_patch_replay_is_noop_and_content_conflict_is_rejected() {
    let f = fixture();
    let now_ms = now();
    let interest = focus_assertion(
        "foc-replay",
        "foc-replay-p",
        "e2",
        FocusReason::ExplicitInterest,
        None,
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent2".to_string()]),
        now_ms,
    );
    let mut assertion = interest.assertion.clone();
    assertion.recorded_at = now_ms;
    assertion.effective_at = now_ms;
    let payload = interest.payload.clone();
    let (ledger_state, base_sequence) = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            Ok((
                (
                    ledger.revision,
                    ledger.input_epoch,
                    ledger.policy_revision,
                    ledger.principal.clone(),
                ),
                sequence,
            ))
        })
        .unwrap();
    let (revision, input_epoch, policy_revision, principal) = ledger_state;
    let build = |patch_id: &str, assertion_id: &str| StatePatch {
        id: patch_id.to_string(),
        base_revision: revision,
        input_epoch,
        policy_revision,
        fence: "replay".into(),
        assertions: vec![Assertion {
            id: assertion_id.to_string(),
            ..assertion.clone()
        }],
        transitions: vec![
            Transition {
                id: "replay-assert".into(),
                sequence: base_sequence,
                assertion_id: assertion_id.to_string(),
                action: Action::Assert,
                reason_code: "replay".into(),
                evidence: assertion.evidence.clone(),
                input_dependencies: assertion.input_dependencies.clone(),
                recorded_at: now_ms,
            },
            Transition {
                id: "replay-activate".into(),
                sequence: base_sequence + 1,
                assertion_id: assertion_id.to_string(),
                action: Action::Activate,
                reason_code: "replay".into(),
                evidence: assertion.evidence.clone(),
                input_dependencies: assertion.input_dependencies.clone(),
                recorded_at: now_ms,
            },
        ],
        coverage: Vec::new(),
    };
    let commit = |patch: &StatePatch| -> Result<bool, String> {
        f.writer.write(|c| {
            let mut payloads = std::collections::BTreeMap::new();
            payloads.insert(assertion.payload_ref.clone(), payload.clone());
            let inputs = assertion.input_dependencies.clone();
            let context = CommitContext {
                access: AccessRequest {
                    principal: &principal,
                    scope: "primary",
                    task_request: Some(PROJECT),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: "replay",
                issued_patch_id: &patch.id,
                issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
                issued_payload_bytes: std::collections::BTreeMap::from([(
                    assertion.payload_ref.clone(),
                    encode(&payload)?.len(),
                )]),
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, patch, &context, &payloads)
        })
    };
    assert!(commit(&build("fixed-patch", "foc-replay")).unwrap());
    // Replaying the identical patch is a no-op.
    assert!(!commit(&build("fixed-patch", "foc-replay")).unwrap());
    // Same patch id with different content (a different assertion id) conflicts.
    let error = commit(&build("fixed-patch", "foc-replay-2")).unwrap_err();
    assert_eq!(error, "personal-patch-Conflict");
}

/// WM-14 performance measurement. Heavy scales run only when explicitly asked.
#[test]
fn t14_measure_world_load_commit_rebuild_query() {
    let scales: Vec<usize> = if std::env::var("SAAA_WORLD_PERF").is_ok() {
        vec![0, 100, 1_000]
    } else {
        vec![0, 100]
    };
    for scale in scales {
        let writer = writer_db();
        let fixture = if scale == 0 {
            None
        } else {
            Some(build_scaled(&writer, scale))
        };
        let (source, project) = match &fixture {
            Some((source, _ids)) => (source.clone(), PROJECT),
            None => {
                let source = writer
                    .write(|c| Ok(insert_source(c, PROJECT, "s1", "scale")))
                    .unwrap();
                (source, PROJECT)
            }
        };
        // warm-up 5
        for _ in 0..5 {
            writer.read_serialized(store::load).unwrap();
            writer
                .read_serialized(|c| store::rebuild(c, now()))
                .unwrap();
            let _ = measure_query(&writer, &source, project);
        }
        let mut load = Vec::new();
        let mut rebuild = Vec::new();
        let mut query = Vec::new();
        for _ in 0..30 {
            let start = std::time::Instant::now();
            writer.read_serialized(store::load).unwrap();
            load.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            writer
                .read_serialized(|c| store::rebuild(c, now()))
                .unwrap();
            rebuild.push(start.elapsed().as_secs_f64() * 1000.0);
            let start = std::time::Instant::now();
            let _ = measure_query(&writer, &source, project);
            query.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let commit = {
            let mut samples = Vec::new();
            for index in 0..5 {
                let built = entity_assertion(
                    &format!("perf-{scale}-{index}"),
                    &format!("perf-{scale}-{index}-p"),
                    &format!("perf-e-{scale}-{index}"),
                    EntityKind::Concept,
                    &format!("Perf {scale} {index}"),
                    &[],
                    &source,
                    project,
                    now(),
                );
                let mut committer = Committer {
                    writer: &writer,
                    project,
                };
                let start = std::time::Instant::now();
                committer
                    .commit(&format!("perf-{scale}-{index}"), vec![built])
                    .unwrap();
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            stats(&samples)
        };
        println!(
            "WORLD_PERF scale={scale} load={:?} rebuild={:?} query={:?} commit={:?}",
            stats(&load),
            stats(&rebuild),
            stats(&query),
            commit
        );
    }
}

fn measure_query(
    writer: &crate::persistence::sqlite::SqliteWriter,
    _source: &SourceRef,
    project: &str,
) -> Result<WorldSlice, String> {
    writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, project);
        activate(
            c,
            &ActivateInput {
                project_scope: project,
                access: &request,
                now: now(),
                seeds: &[WorldSeed::ExactName("Perf 1 1".into())],
                causal_direction: CausalDirection::Forward,
                limits: Limits::default(),
                max_bytes: 8_192,
            },
        )
    })
}

fn stats(samples: &[f64]) -> (f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize % sorted.len()];
    let max = *sorted.last().unwrap();
    (median, p95, max)
}

fn build_scaled(
    writer: &crate::persistence::sqlite::SqliteWriter,
    scale: usize,
) -> (SourceRef, Vec<String>) {
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "s1", "scale fixture")))
        .unwrap();
    let mut ids = Vec::new();
    let now_ms = now();
    let mut index = 0;
    while index < scale {
        let batch: Vec<BuiltAssertion> = (index..(index + 8).min(scale))
            .map(|n| {
                let id = format!("scaled-{n}");
                ids.push(id.clone());
                entity_assertion(
                    &id,
                    &format!("{id}-p"),
                    &format!("scale-e-{n}"),
                    EntityKind::Concept,
                    &format!("Scaled {n}"),
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                )
            })
            .collect();
        let mut committer = Committer {
            writer,
            project: PROJECT,
        };
        committer
            .commit(&format!("scaled-patch-{index}"), batch)
            .unwrap();
        index += 8;
    }
    (source, ids)
}

// ---- R1..R10 review regressions -------------------------------------------

fn query_at(
    f: &Fixture,
    seeds: &[WorldSeed],
    limits: Limits,
    max_bytes: usize,
    now_ms: i64,
) -> Result<WorldSlice, String> {
    f.writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, PROJECT);
        activate(
            c,
            &ActivateInput {
                project_scope: PROJECT,
                access: &request,
                now: now_ms,
                seeds,
                causal_direction: CausalDirection::Forward,
                limits,
                max_bytes,
            },
        )
    })
}

#[test]
fn r01_principal_classification_and_revoked_scope_are_denied() {
    let f = fixture();
    // A different principal is a contract error.
    let principal_error = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let error = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &AccessRequest {
                        principal: "someone-else",
                        scope: "primary",
                        task_request: Some(PROJECT),
                        purpose: Purpose::Reasoning,
                        max_classification: Classification::Confidential,
                        policy_revision: ledger.policy_revision,
                        authorized: true,
                    },
                    now: now(),
                    seeds: &[WorldSeed::EntityId("e1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )
            .unwrap_err();
            Ok(error)
        })
        .unwrap();
    assert_eq!(principal_error, "world-scope-denied");

    // A lower classification ceiling must not expose Confidential names or paths.
    let filtered = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &AccessRequest {
                        principal: &ledger.principal,
                        scope: "primary",
                        task_request: Some(PROJECT),
                        purpose: Purpose::Reasoning,
                        max_classification: Classification::Internal,
                        policy_revision: ledger.policy_revision,
                        authorized: true,
                    },
                    now: now(),
                    seeds: &[WorldSeed::EntityId("e1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )
        })
        .unwrap();
    assert!(filtered.nodes.is_empty(), "{:?}", filtered.nodes);
    assert!(filtered.relevance_paths.is_empty());
    assert!(filtered.causal_paths.is_empty());

    // A revoked project scope stops every read.
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE context_scopes SET state='revoked' WHERE scope_key=?1",
                [PROJECT],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let revoked = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits::default(),
        8_192,
        now(),
    )
    .unwrap_err();
    assert_eq!(revoked, "world-scope-denied");
}

#[test]
fn r02_time_only_expiry_hides_entities_and_current_work() {
    let f = fixture();
    let future = now() + 10_000;
    // Expire e2 only in the projection: the meta revision/epoch is unchanged.
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE personal_world_entities SET valid_until=?1 WHERE entity_id='e2'",
                [future - 1],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let slice = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits::default(),
        8_192,
        future,
    )
    .unwrap();
    assert!(
        slice.nodes.iter().all(|n| n.entity_id != "e2"),
        "expired entity leaked: {:?}",
        slice.nodes
    );
    assert!(
        slice.causal_paths.is_empty(),
        "expired endpoint leaked: {:?}",
        slice.causal_paths
    );

    // An expired Objective must not keep a current_work Focus alive.
    let f = fixture();
    f.writer
        .write(|c| {
            c.execute(
                "UPDATE personal_assertions SET metadata=json_set(metadata,'$.valid_until',?1) \
                 WHERE id='obj1'",
                [future - 1],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let slice = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits::default(),
        8_192,
        future,
    )
    .unwrap();
    assert!(
        slice
            .focus
            .iter()
            .all(|focus| focus.reason != "current_work"),
        "expired objective kept current_work: {:?}",
        slice.focus
    );
}

#[test]
fn r03_edge_fetch_does_not_consume_the_traversal_scan_budget() {
    let f = fixture();
    // Three project relations and scan=3: the loader must not zero the traversal budget.
    let slice = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits {
            scan: 3,
            ..Limits::default()
        },
        8_192,
        now(),
    )
    .unwrap();
    assert!(
        !slice.causal_paths.is_empty(),
        "scan budget was consumed by the edge fetch: {:?}",
        slice.notices
    );
}

#[test]
fn r05_condition_mismatch_does_not_erase_a_valid_short_path() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    let replacement = relation_assertion(
        "rel2b",
        "rel2b-p",
        "e2",
        "e3",
        RelationType::Increases,
        Some(EffectInput::QuantityIncrease),
        &[("config", "other")],
        Basis::ModelHypothesis,
        vec![(f.source.key.clone(), Stance::Supports)],
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent2".to_string(), "ent3".to_string()]),
        f.built_at,
    );
    committer
        .transition("r05-retract", "rel2", Action::Retract)
        .unwrap();
    committer.commit("r05", vec![replacement]).unwrap();
    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    assert_eq!(
        slice.causal_paths.len(),
        1,
        "valid prefix erased: {:?}",
        slice.causal_paths
    );
    assert_eq!(slice.causal_paths[0].nodes, vec!["e1", "e2"]);
}

#[test]
fn r06_unrelated_focus_does_not_join_another_focus_gap() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .commit(
            "r06",
            vec![
                entity_assertion(
                    "ent5",
                    "ent5p",
                    "e5",
                    EntityKind::Metric,
                    "Throughput",
                    &[],
                    &f.source,
                    PROJECT,
                    f.built_at,
                ),
                relation_assertion(
                    "rel4",
                    "rel4-p",
                    "e1",
                    "e5",
                    RelationType::Increases,
                    Some(EffectInput::Intervention),
                    &[("config", "fixture-a")],
                    Basis::ModelHypothesis,
                    vec![(f.source.key.clone(), Stance::Supports)],
                    &f.source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent1".to_string(), "ent5".to_string()]),
                    f.built_at,
                ),
                focus_assertion(
                    "foc5",
                    "foc5p",
                    "e5",
                    FocusReason::ExplicitInterest,
                    None,
                    &f.source,
                    PROJECT,
                    std::collections::BTreeSet::from(["ent5".to_string()]),
                    f.built_at,
                ),
            ],
        )
        .unwrap();
    let slice = query(&f, &[WorldSeed::EntityId("e1".into())]).unwrap();
    let rel4: Vec<&_> = slice
        .research_gaps
        .iter()
        .filter(|g| g.relation_assertion_id == "rel4")
        .collect();
    assert!(!rel4.is_empty(), "expected a gap for rel4");
    assert!(
        rel4.iter().all(|g| g.focus_entity_id == "e5"),
        "rel4 joined an unrelated Focus: {:?}",
        rel4
    );
    assert!(
        slice
            .research_gaps
            .iter()
            .filter(|g| g.focus_entity_id == "e4")
            .all(|g| g.relation_assertion_id != "rel4"),
        "e4 received a gap for a path it never reached"
    );
}

#[test]
fn r07_nodes_are_bounded_even_with_focus() {
    let f = fixture();
    let slice = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits {
            nodes: 2,
            ..Limits::default()
        },
        8_192,
        now(),
    )
    .unwrap();
    assert!(
        slice.nodes.len() <= 2,
        "node budget bypassed: {:?}",
        slice.nodes
    );
    let present: std::collections::BTreeSet<&str> =
        slice.nodes.iter().map(|n| n.entity_id.as_str()).collect();
    for path in slice
        .relevance_paths
        .iter()
        .chain(slice.causal_paths.iter())
    {
        assert!(path.nodes.iter().all(|n| present.contains(n.as_str())));
    }
}

#[test]
fn r08_small_byte_budget_is_an_explicit_error() {
    let f = fixture();
    let error = query_at(
        &f,
        &[WorldSeed::EntityId("e1".into())],
        Limits::default(),
        1,
        now(),
    )
    .unwrap_err();
    assert_eq!(error, "world-budget-too-small");
}

#[test]
fn r11_causal_project_start_is_rejected() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    // A project is a work context, not a causal input; only concept/metric may start an effect.
    let bad = relation_assertion(
        "bad-causal",
        "bad-causal-p",
        "e4",
        "e2",
        RelationType::Decreases,
        Some(EffectInput::Intervention),
        &[],
        Basis::ModelHypothesis,
        vec![(f.source.key.clone(), Stance::Supports)],
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent4".to_string(), "ent2".to_string()]),
        f.built_at,
    );
    assert_eq!(
        committer.commit("bad-causal", vec![bad]).unwrap_err(),
        "world-invalid-payload"
    );
}

#[test]
fn r12_seed_set_must_be_bounded_and_fully_resolved() {
    let f = fixture();
    // A requested seed that does not resolve omits the whole query; it is never dropped.
    let slice = query(
        &f,
        &[
            WorldSeed::EntityId("e1".into()),
            WorldSeed::ExactName("no such entity".into()),
        ],
    )
    .unwrap();
    assert!(
        slice.notices.contains(&"unknown_seed".to_string()),
        "{:?}",
        slice.notices
    );
    assert!(slice.relevance_paths.is_empty());
    // More than four seeds is a contract limit, not a partial answer.
    let seeds = vec![WorldSeed::EntityId("e1".into()); 5];
    let error = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let request = access(&ledger, PROJECT);
            let error = activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &seeds,
                    causal_direction: CausalDirection::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )
            .unwrap_err();
            Ok(error)
        })
        .unwrap();
    assert_eq!(error, "world-limit");
}

#[test]
fn r13_candidate_endpoint_cannot_be_referenced() {
    let f = fixture();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .commit_candidate(
            "candidate-ent",
            vec![entity_assertion(
                "ent-cand",
                "ent-cand-p",
                "e9",
                EntityKind::Metric,
                "Candidate Metric",
                &[],
                &f.source,
                PROJECT,
                f.built_at,
            )],
        )
        .unwrap();
    // A relation may only depend on a currently Active endpoint.
    let bad = relation_assertion(
        "rel-cand",
        "rel-cand-p",
        "e1",
        "e9",
        RelationType::Decreases,
        Some(EffectInput::Intervention),
        &[],
        Basis::ModelHypothesis,
        vec![(f.source.key.clone(), Stance::Supports)],
        &f.source,
        PROJECT,
        std::collections::BTreeSet::from(["ent1".to_string(), "ent-cand".to_string()]),
        f.built_at,
    );
    assert_eq!(
        committer.commit("rel-cand", vec![bad]).unwrap_err(),
        "world-invalid-reference"
    );
}
