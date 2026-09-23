use super::*;
#[test]
pub(super) fn d36_prepare_outcome_patch_has_dispute_transitions_and_no_prior_dependency() {
    let f = fixture_v2();
    let (prediction, outcome) = counterexample(
        &f.source,
        EffectDirection::Increase,
        EffectDirection::Decrease,
    );
    let prepared = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            prepare_outcome_patch(c, &ledger, PROJECT, "rel_inc", &prediction, &outcome, now())
        })
        .unwrap();
    assert!(prepared.operation_key.starts_with("wmo2:"));
    let assertion = &prepared.patch.assertions[0];
    assert!(!assertion.depends_on.contains("rel_inc"));
    assert!(assertion.input_dependencies.contains(&f.source.key));
    let actions: Vec<String> = prepared
        .patch
        .transitions
        .iter()
        .map(|t| match &t.action {
            Action::Assert => "assert".to_string(),
            Action::Activate => "activate".to_string(),
            Action::Dispute => "dispute".to_string(),
            Action::Supersede { .. } => "supersede".to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(actions, vec!["assert", "supersede", "activate", "dispute"]);
}
#[test]
pub(super) fn d37_outcome_resend_is_a_noop_and_second_outcome_is_rejected() {
    let f = fixture_v2();
    let (prediction, outcome) = counterexample(
        &f.source,
        EffectDirection::Increase,
        EffectDirection::Decrease,
    );
    let prepared = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            prepare_outcome_patch(c, &ledger, PROJECT, "rel_inc", &prediction, &outcome, now())
        })
        .unwrap();
    assert!(commit_prepared_outcome(&f.writer, &prepared, "fence-outcome").unwrap());
    let revision: u64 = f
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT revision FROM personal_scope WHERE id='primary'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    // Re-send: same prepared content is a no-op.
    assert!(!commit_prepared_outcome(&f.writer, &prepared, "fence-outcome").unwrap());
    let after: u64 = f
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT revision FROM personal_scope WHERE id='primary'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(revision, after);
    // The prior version is now superseded, so a second outcome cannot reuse it.
    let second = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            prepare_outcome_patch(c, &ledger, PROJECT, "rel_inc", &prediction, &outcome, now())
        })
        .unwrap_err();
    assert_eq!(second, "world-invalid-reference");
}
/// D41 performance fixture. Run explicitly:
/// `cargo test --lib d41_v2_performance -- --ignored --nocapture`
/// The 10,000 scale is outside the interrupt budget by design in M1.
/// M2-01: requested / created are reported separately, every measured
/// operation must succeed, and the p95 is the nearest-rank 29th of 30 samples.
#[test]
#[ignore]
pub(super) fn d41_v2_performance() {
    use std::time::Instant;
    let scales: [usize; 3] = [0, 100, 1_000];
    for scale in scales {
        let requested = scale;
        let writer = writer_db();
        let now_ms = now();
        let source = writer
            .write(|c| Ok(insert_source(c, PROJECT, "perf", "perf source")))
            .unwrap();
        let key = source.key.clone();
        let mut committer = Committer {
            writer: &writer,
            project: PROJECT,
        };
        let build_start = Instant::now();
        let mut created = 0usize;
        let mut interrupted = false;
        while created < scale {
            if build_start.elapsed().as_secs() > 5 {
                interrupted = true;
                break;
            }
            let batch: Vec<_> = (created..(created + 8).min(scale))
                .map(|index| {
                    v2_entity_assertion(
                        &format!("pe{index}"),
                        &format!("peer{index}"),
                        EntityKindV2::Concept,
                        &format!("概念{index}"),
                        &[],
                        None,
                        &source,
                        PROJECT,
                        now_ms,
                    )
                })
                .collect();
            let batch_len = batch.len();
            committer.commit(&format!("perf-{created}"), batch).unwrap();
            created += batch_len;
        }
        let build = build_start.elapsed();
        assert!(
            interrupted || created == requested,
            "created {created} != requested {requested}"
        );
        let measure = |label: &str, mut op: Box<dyn FnMut() -> u128>| {
            for _ in 0..5 {
                let _ = op();
            }
            let mut samples = Vec::with_capacity(30);
            for _ in 0..30 {
                samples.push(op());
            }
            samples.sort();
            // nearest-rank p95 for 30 samples is the 29th ascending value.
            let p95 = samples[(30f64 * 0.95).ceil() as usize - 1];
            eprintln!(
                "WORLD_V2_PERF scale={scale} requested={requested} created={created} \
                 interrupted={interrupted} {label} p95_ms={} max_ms={}",
                p95 as f64 / 1000.0,
                samples[samples.len() - 1] as f64 / 1000.0
            );
        };
        eprintln!(
            "WORLD_V2_PERF scale={scale} requested={requested} created={created} \
             interrupted={interrupted} build_ms={}",
            build.as_millis()
        );
        measure(
            "load",
            Box::new(|| {
                let start = Instant::now();
                writer
                    .read_serialized(|c| {
                        store::load(c)?;
                        Ok(())
                    })
                    .expect("load succeeds");
                start.elapsed().as_micros()
            }),
        );
        measure(
            "rebuild",
            Box::new(|| {
                let start = Instant::now();
                writer
                    .write(|c| {
                        store::rebuild(c, now())?;
                        Ok(())
                    })
                    .expect("rebuild succeeds");
                start.elapsed().as_micros()
            }),
        );
        measure(
            "query",
            Box::new(|| {
                let start = Instant::now();
                run_v2(
                    &writer,
                    &[WorldSeed::EntityId("peer0".into())],
                    IncludeFlags::default(),
                    8_192,
                    now(),
                )
                .expect("query succeeds");
                start.elapsed().as_micros()
            }),
        );
        let _ = key;
    }
}
/// M2-01: nearest-rank p95 for exactly 30 samples is the 29th ascending value.
#[test]
pub(super) fn m2_01_nearest_rank_p95_method() {
    let mut samples: Vec<u128> = (1..=30).collect();
    samples.sort();
    let p95 = samples[(30f64 * 0.95).ceil() as usize - 1];
    assert_eq!(p95, 29);
    assert_eq!(samples.len(), 30);
}
#[test]
pub(super) fn d20_v1_reader_treats_a_v2_projection_as_stale() {
    use super::super::query::{activate, ActivateInput};
    use saaa_personal_state_core::world::traversal::{CausalDirection as V1Direction, Limits};
    let f = fixture_v2();
    let slice = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let request = access(&ledger, PROJECT);
            activate(
                c,
                &ActivateInput {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("c1".into())],
                    causal_direction: V1Direction::Forward,
                    limits: Limits::default(),
                    max_bytes: 8_192,
                },
            )
        })
        .unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice
        .notices
        .iter()
        .any(|notice| notice == "projection_stale"));
}
#[test]
pub(super) fn d38_focus_is_present_in_the_slice() {
    let f = fixture_v2();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(
        slice.focus.iter().any(|focus| focus.entity_id == "g1"),
        "current_work focus must be returned"
    );
    assert!(slice.relevant_goal_ids.iter().any(|id| id == "g1"));
}
#[test]
pub(super) fn world_review_final_slice_keeps_derived_path_metadata() {
    let f = fixture_v2();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .commit(
            "correlation-entity",
            vec![v2_entity_assertion(
                "ent_m2",
                "m2",
                EntityKindV2::Metric,
                "Throughput",
                &[],
                None,
                &f.source,
                PROJECT,
                f.now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "correlation-metadata",
            vec![v2_relation_assertion(
                "rel_corr",
                v2_relation_value(
                    "m1",
                    "m2",
                    "correlates_with",
                    None,
                    &[],
                    None,
                    None,
                    Some("positive"),
                    None,
                    std::slice::from_ref(&f.source.key),
                    None,
                    None,
                ),
                &f.source,
                PROJECT,
                BTreeSet::from(["ent_m1".into(), "ent_m2".into()]),
                f.now_ms,
            )],
        )
        .unwrap();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(!slice.causal_paths.is_empty());
    assert!(!slice.effect_summaries.is_empty());
    assert!(slice
        .effect_summaries
        .iter()
        .flat_map(|summary| &summary.path_indices)
        .all(|index| *index < slice.causal_paths.len()));
    assert!(slice.correlation_ids.iter().any(|id| id == "rel_corr"));
}
#[test]
pub(super) fn world_review_limits_apply_after_access_and_flag_filters() {
    let f = fixture_v2();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .commit(
            "denied-before-limit",
            vec![
                v2_relation_assertion(
                    "aaa_denied_relation",
                    v2_relation_value(
                        "c1",
                        "p1",
                        "related_to",
                        None,
                        &[],
                        None,
                        None,
                        None,
                        None,
                        std::slice::from_ref(&f.source.key),
                        None,
                        None,
                    ),
                    &f.source,
                    PROJECT,
                    BTreeSet::from(["ent_c1".into(), "ent_p1".into()]),
                    f.now_ms,
                ),
                v2_focus_assertion(
                    "aaa_denied_focus_c1",
                    "c1",
                    "current_work",
                    Some("obj1"),
                    &f.source,
                    PROJECT,
                    BTreeSet::from(["ent_c1".into(), "obj1".into()]),
                    f.now_ms,
                ),
                v2_focus_assertion(
                    "aaa_denied_focus_m1",
                    "m1",
                    "current_work",
                    Some("obj1"),
                    &f.source,
                    PROJECT,
                    BTreeSet::from(["ent_m1".into(), "obj1".into()]),
                    f.now_ms,
                ),
            ],
        )
        .unwrap();
    f.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE personal_assertions
                        SET metadata=json_set(metadata,'$.access.purposes',json('[\"diagnostics\"]'))
                      WHERE id IN ('aaa_denied_relation','aaa_denied_focus_c1','aaa_denied_focus_m1')",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let slice = f
        .writer
        .read_serialized(|connection| {
            let ledger = store::load(connection)?;
            let request = access(&ledger, PROJECT);
            let mut limits = LimitsV2::m1();
            limits.fetch_rows = 1;
            limits.nodes = 2;
            activate_v2(
                connection,
                &ActivateInputV2 {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("c1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits,
                    max_bytes: 8_192,
                    request_id: "review-filter-limit",
                    explicit_question: false,
                    flags: IncludeFlags::default(),
                    condition_observations: &[],
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    assert!(slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "rel_inc"));
    assert!(slice.focus.iter().any(|focus| focus.entity_id == "g1"));
    assert!(!slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "aaa_denied_relation"));
}
#[test]
pub(super) fn d28_unknown_explicit_seed_yields_missing_knowledge_gap() {
    use super::super::query_v2::IncludeFlags as Flags;
    let f = fixture_v2();
    let slice = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let request = access(&ledger, PROJECT);
            activate_v2(
                c,
                &ActivateInputV2 {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::ExactName("未登録の話題".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: LimitsV2::m1(),
                    max_bytes: 8_192,
                    request_id: "req-unknown",
                    explicit_question: true,
                    flags: Flags::default(),
                    condition_observations: &[],
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    assert!(slice.nodes.is_empty());
    let gap = slice
        .research_gaps
        .iter()
        .find(|gap| {
            gap.kind == saaa_personal_state_core::world::slice_v2::GapKindV2::MissingKnowledge
        })
        .expect("missing_knowledge gap");
    assert_eq!(gap.subject.unresolved_seed.as_deref(), Some("未登録の話題"));
    assert!(gap.subject.relation_ids.is_empty());
}
#[test]
pub(super) fn d21_activate_v2_reads_a_v1_only_projection() {
    use saaa_personal_state_core::world::EntityKind;
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "v1only", "v1 only source")))
        .unwrap();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "v1-only",
            vec![entity_assertion(
                "v1ent",
                "v1entp",
                "v1e1",
                EntityKind::Concept,
                "V1 Concept",
                &[],
                &source,
                PROJECT,
                now_ms,
            )],
        )
        .unwrap();
    // The projection is v1-only (projection_version = 1), but activate_v2 reads it.
    let slice = run_v2(
        &writer,
        &[WorldSeed::EntityId("v1e1".into())],
        IncludeFlags::default(),
        8_192,
        now(),
    )
    .unwrap();
    assert!(slice.nodes.iter().any(|node| node.entity_id == "v1e1"));
}
pub(crate) struct ChainFixture {
    pub(super) writer: SqliteWriter,
    pub(super) source: SourceRef,
    pub(super) now_ms: i64,
}
