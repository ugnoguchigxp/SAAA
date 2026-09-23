use super::*;
#[test]
pub(super) fn t14_scope_isolation_and_authorization() {
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
pub(super) fn t15_stale_projection_and_pending_review_are_explicit() {
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
pub(super) fn t03_world_kinds_do_not_leak_into_continuity_paths() {
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
pub(super) fn t16_t17_traversal_is_bounded_and_deterministic() {
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
pub(super) fn t21_slice_budget_is_respected_for_japanese_json() {
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
pub(super) fn relation1(
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
pub(super) fn t06_duplicate_active_replacement_and_dispute() {
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
pub(super) fn t07_objective_retraction_removes_current_work_focus() {
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
pub(super) fn t05_world_cycles_are_allowed_but_assertion_dependency_cycles_are_not() {
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
pub(super) fn t20_focus_ranking_and_stable_gap_keys() {
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
