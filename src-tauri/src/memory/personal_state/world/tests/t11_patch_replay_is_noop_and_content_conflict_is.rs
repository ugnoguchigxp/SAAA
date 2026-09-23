use super::*;
#[test]
pub(super) fn t11_patch_replay_is_noop_and_content_conflict_is_rejected() {
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
pub(super) fn t14_measure_world_load_commit_rebuild_query() {
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
pub(super) fn measure_query(
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
pub(super) fn stats(samples: &[f64]) -> (f64, f64, f64) {
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
pub(super) fn build_scaled(
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

pub(crate) fn query_at(
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
pub(super) fn r01_principal_classification_and_revoked_scope_are_denied() {
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
pub(super) fn r02_time_only_expiry_hides_entities_and_current_work() {
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
pub(super) fn r03_edge_fetch_does_not_consume_the_traversal_scan_budget() {
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
pub(super) fn r05_condition_mismatch_does_not_erase_a_valid_short_path() {
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
