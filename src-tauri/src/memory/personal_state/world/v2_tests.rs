#![cfg(test)]

//! v2 adapter integration tests (D21/D23/D31/D32/D38/D39). Deterministic
//! fixtures only; no model, network or production DB.

use super::query::WorldSeed;
use super::query_v2::{activate_v2, activate_v2_with_stats, ActivateInputV2, IncludeFlags};
use super::test_support::*;
use crate::memory::personal_state::{now, store};
use crate::persistence::sqlite::SqliteWriter;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::slice_v2::WorldSliceV2;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::*;
use serde_json::json;
use std::collections::BTreeSet;

struct V2Fixture {
    writer: SqliteWriter,
    source: SourceRef,
    now_ms: i64,
}

fn fixture_v2() -> V2Fixture {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "s1", "v2 fixture source")))
        .unwrap();
    let key = source.key.clone();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "v2-objective",
            vec![objective_assertion(
                "obj1", "obj1p", &source, PROJECT, now_ms,
            )],
        )
        .unwrap();
    committer
        .commit(
            "v2-entities",
            vec![
                v2_entity_assertion(
                    "ent_p1",
                    "p1",
                    EntityKindV2::Project,
                    "SAAA",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
                v2_entity_assertion(
                    "ent_c1",
                    "c1",
                    EntityKindV2::Concept,
                    "Speculative Decoding",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
                v2_entity_assertion(
                    "ent_m1",
                    "m1",
                    EntityKindV2::Metric,
                    "Decode Latency",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
                v2_entity_assertion(
                    "ent_g1",
                    "g1",
                    EntityKindV2::Goal,
                    "Natural Conversation",
                    &[],
                    Some("obj1"),
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    let relation = |from: &str,
                    to: &str,
                    relation_type: &str,
                    effect: Option<&str>,
                    target: Option<&str>,
                    comparison: Option<&str>,
                    confidence: Option<(u16, &str)>| {
        v2_relation_value(
            from,
            to,
            relation_type,
            effect,
            &[("config", "a")],
            comparison,
            target,
            None,
            confidence,
            std::slice::from_ref(&key),
            None,
            None,
        )
    };
    let depends =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| id.to_string()).collect() };
    committer
        .commit(
            "v2-relations",
            vec![
                v2_relation_assertion(
                    "rel_inc",
                    relation(
                        "c1",
                        "m1",
                        "increases",
                        Some("intervention"),
                        None,
                        Some("cmp"),
                        Some((800, "manual_v1")),
                    ),
                    &source,
                    PROJECT,
                    depends(&["ent_c1", "ent_m1"]),
                    now_ms,
                ),
                v2_relation_assertion(
                    "rel_sg",
                    relation(
                        "m1",
                        "g1",
                        "serves_goal",
                        None,
                        Some("lower_is_better"),
                        None,
                        None,
                    ),
                    &source,
                    PROJECT,
                    depends(&["ent_m1", "ent_g1"]),
                    now_ms,
                ),
                v2_relation_assertion(
                    "rel_hg",
                    relation("p1", "g1", "has_goal", None, None, None, None),
                    &source,
                    PROJECT,
                    depends(&["ent_p1", "ent_g1"]),
                    now_ms,
                ),
            ],
        )
        .unwrap();
    committer
        .commit(
            "v2-focus",
            vec![v2_focus_assertion(
                "focus_g1",
                "g1",
                "current_work",
                Some("obj1"),
                &source,
                PROJECT,
                depends(&["ent_g1", "obj1"]),
                now_ms,
            )],
        )
        .unwrap();
    V2Fixture {
        writer,
        source,
        now_ms,
    }
}

fn run_v2(
    writer: &SqliteWriter,
    seeds: &[WorldSeed],
    flags: IncludeFlags,
    max_bytes: usize,
    _now_ms: i64,
) -> Result<WorldSliceV2, String> {
    writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, PROJECT);
        activate_v2(
            c,
            &ActivateInputV2 {
                project_scope: PROJECT,
                access: &request,
                now: now(),
                seeds,
                causal_direction: CausalDirection::Forward,
                limits: LimitsV2::m1(),
                max_bytes,
                request_id: "req-1",
                explicit_question: false,
                flags,
                condition_observations: &[],
                availability_observations: &[],
                temporary_attention_entity_ids: &[],
            },
        )
    })
}

#[test]
fn d21_unknown_seed_is_a_notice_not_a_fabricated_node() {
    let f = fixture_v2();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::ExactName("does-not-exist".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice.notices.iter().any(|n| n == "unknown_seed"));
}

#[test]
fn v2_forgotten_source_version_is_not_revived_by_a_new_version_with_the_same_id() {
    let f = fixture_v2();
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

    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice.relations.is_empty());
}

#[test]
fn d21_stale_projection_is_an_omission_not_an_error() {
    let writer = writer_db();
    writer
        .write(|c| {
            insert_source(c, PROJECT, "s1", "stale source");
            Ok(())
        })
        .unwrap();
    let slice = run_v2(
        &writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        now(),
    )
    .unwrap();
    assert!(slice.notices.iter().any(|n| n == "projection_stale"));
}

#[test]
fn d21_query_is_read_only() {
    let f = fixture_v2();
    let before: u64 = f
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
    let _ = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
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
    assert_eq!(before, after);
}

#[test]
fn d31_two_hop_relation_is_fetched_from_the_second_frontier() {
    let f = fixture_v2();
    let (slice, stats) = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            let request = access(&ledger, PROJECT);
            activate_v2_with_stats(
                c,
                &ActivateInputV2 {
                    project_scope: PROJECT,
                    access: &request,
                    now: now(),
                    seeds: &[WorldSeed::EntityId("c1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: LimitsV2::m1(),
                    max_bytes: 8_192,
                    request_id: "req-1",
                    explicit_question: false,
                    flags: IncludeFlags::default(),
                    condition_observations: &[],
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    assert!(stats.fetch_rows > 0);
    assert!(stats.fetch_rows <= 500);
    assert!(stats.scan_steps <= 500);
    // m1 -> g1 is two hops from c1, so the frontier fetch reached depth 2.
    assert!(slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "rel_sg"));
}

#[test]
fn d32_flags_remove_disabled_relation_kinds() {
    let f = fixture_v2();
    let flags = IncludeFlags {
        causal: false,
        ..IncludeFlags::default()
    };
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        flags,
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(!slice.relations.iter().any(|relation| matches!(
        relation.relation_type,
        saaa_personal_state_core::world::model_v2::RelationTypeV2::Increases
            | saaa_personal_state_core::world::model_v2::RelationTypeV2::Decreases
    )));
}

#[test]
fn d32_goals_flag_removes_goal_relations_and_nodes() {
    let f = fixture_v2();
    let flags = IncludeFlags {
        goals: false,
        ..IncludeFlags::default()
    };
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        flags,
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(!slice.relations.iter().any(|relation| matches!(
        relation.relation_type,
        saaa_personal_state_core::world::model_v2::RelationTypeV2::HasGoal
            | saaa_personal_state_core::world::model_v2::RelationTypeV2::ServesGoal
    )));
    assert!(!slice
        .nodes
        .iter()
        .any(|node| node.entity_kind == EntityKindV2::Goal));
}

#[test]
fn d38_goal_is_reached_through_a_relevance_path() {
    let f = fixture_v2();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(slice
        .nodes
        .iter()
        .any(|node| node.entity_id == "g1" && node.entity_kind == EntityKindV2::Goal));
    assert!(!slice.relevance_paths.is_empty());
    // The only causal edge is c1 -> m1: one hop.
    assert!(slice.causal_paths.iter().all(|path| path.hops <= 1));
}

#[test]
fn d38_goal_retraction_removes_current_importance() {
    let f = fixture_v2();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .transition("v2-retract", "obj1", Action::Retract)
        .unwrap();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::EntityId("c1".into())],
        IncludeFlags::default(),
        8_192,
        f.now_ms,
    )
    .unwrap();
    assert!(!slice.nodes.iter().any(|node| node.entity_id == "g1"));
    assert!(!slice.focus.iter().any(|focus| focus.entity_id == "g1"));
}

#[test]
fn d39_condition_observation_makes_the_relation_satisfied() {
    use super::observations_v2::ConditionObservationInput;
    let f = fixture_v2();
    let observation = ConditionObservationInput {
        relation_assertion_id: "rel_inc".into(),
        key: "config".into(),
        value: "a".into(),
        source: f.source.key.clone(),
    };
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
                    seeds: &[WorldSeed::EntityId("c1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: LimitsV2::m1(),
                    max_bytes: 8_192,
                    request_id: "req-1",
                    explicit_question: false,
                    flags: IncludeFlags::default(),
                    condition_observations: std::slice::from_ref(&observation),
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    let relation = slice
        .relations
        .iter()
        .find(|relation| relation.assertion_id == "rel_inc")
        .expect("rel_inc present");
    assert_eq!(
        relation.condition_state,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Satisfied
    );
    assert!(slice
        .causal_paths
        .iter()
        .any(|path| path.confidence.is_some() && path.direction.as_str() == "increase"));
    let _ = json!(null);
}

use super::outcome_v2::{commit_prepared_outcome, prepare_outcome_patch};
use saaa_personal_state_core::world::model_v2::EffectDirection;
use saaa_personal_state_core::world::outcome_v2::{Outcome, Prediction};

fn counterexample(
    source: &SourceRef,
    expected: EffectDirection,
    actual: EffectDirection,
) -> (Prediction, Outcome) {
    let conditions = vec![("config".to_string(), "a".to_string())];
    let prediction = Prediction {
        metric_id: "m1".into(),
        comparison_id: Some("cmp".into()),
        conditions: conditions.clone(),
        direction: expected,
        at_ms: now(),
        source: source.key.clone(),
    };
    let outcome = Outcome {
        metric_id: "m1".into(),
        comparison_id: Some("cmp".into()),
        conditions,
        direction: actual,
        at_ms: now() + 1,
        source: source.key.clone(),
    };
    (prediction, outcome)
}

#[test]
fn d36_prepare_outcome_patch_has_dispute_transitions_and_no_prior_dependency() {
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
fn d37_outcome_resend_is_a_noop_and_second_outcome_is_rejected() {
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
fn d41_v2_performance() {
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
fn m2_01_nearest_rank_p95_method() {
    let mut samples: Vec<u128> = (1..=30).collect();
    samples.sort();
    let p95 = samples[(30f64 * 0.95).ceil() as usize - 1];
    assert_eq!(p95, 29);
    assert_eq!(samples.len(), 30);
}

#[test]
fn d20_v1_reader_treats_a_v2_projection_as_stale() {
    use super::query::{activate, ActivateInput};
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
fn d38_focus_is_present_in_the_slice() {
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
fn d28_unknown_explicit_seed_yields_missing_knowledge_gap() {
    use super::query_v2::IncludeFlags as Flags;
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
fn d21_activate_v2_reads_a_v1_only_projection() {
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

struct ChainFixture {
    writer: SqliteWriter,
    source: SourceRef,
    now_ms: i64,
}

fn fixture_chain_v2() -> ChainFixture {
    let writer = writer_db();
    let now_ms = now();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "chain", "chain source")))
        .unwrap();
    let key = source.key.clone();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "chain-entities",
            vec![
                v2_entity_assertion(
                    "ce_c0",
                    "c0",
                    EntityKindV2::Concept,
                    "Intervention",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
                v2_entity_assertion(
                    "ce_m8",
                    "m8",
                    EntityKindV2::Metric,
                    "Mid Latency",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
                v2_entity_assertion(
                    "ce_m9",
                    "m9",
                    EntityKindV2::Metric,
                    "Final Latency",
                    &[],
                    None,
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    let depends =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| id.to_string()).collect() };
    committer
        .commit(
            "chain-relations",
            vec![
                v2_relation_assertion(
                    "chain_a",
                    v2_relation_value(
                        "c0",
                        "m8",
                        "increases",
                        Some("intervention"),
                        &[("config", "a")],
                        Some("cmp"),
                        None,
                        None,
                        Some((800, "manual_v1")),
                        std::slice::from_ref(&key),
                        None,
                        None,
                    ),
                    &source,
                    PROJECT,
                    depends(&["ce_c0", "ce_m8"]),
                    now_ms,
                ),
                v2_relation_assertion(
                    "chain_b",
                    v2_relation_value(
                        "m8",
                        "m9",
                        "increases",
                        Some("quantity_increase"),
                        &[("config", "a")],
                        Some("cmp"),
                        None,
                        None,
                        Some((800, "manual_v1")),
                        std::slice::from_ref(&key),
                        None,
                        None,
                    ),
                    &source,
                    PROJECT,
                    depends(&["ce_m8", "ce_m9"]),
                    now_ms,
                ),
            ],
        )
        .unwrap();
    ChainFixture {
        writer,
        source,
        now_ms,
    }
}

#[test]
fn d25_reverse_causal_search_composes_in_declared_order() {
    use super::observations_v2::ConditionObservationInput;
    let f = fixture_chain_v2();
    let observations = ["chain_a", "chain_b"].map(|relation| ConditionObservationInput {
        relation_assertion_id: relation.into(),
        key: "config".into(),
        value: "a".into(),
        source: f.source.key.clone(),
    });
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
                    seeds: &[WorldSeed::EntityId("m9".into())],
                    causal_direction: CausalDirection::Reverse,
                    limits: LimitsV2::m1(),
                    max_bytes: 8_192,
                    request_id: "req-rev",
                    explicit_question: false,
                    flags: IncludeFlags::default(),
                    condition_observations: &observations,
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    let path = slice
        .causal_paths
        .iter()
        .find(|path| path.hops == 2)
        .expect("two-hop reverse path");
    assert_eq!(
        path.direction.as_str(),
        "increase",
        "declared c0->m8->m9 is two increases"
    );
    assert_eq!(path.confidence.as_ref().map(|c| c.value), Some(720));
    let _ = f.now_ms;
}

#[test]
fn d32_all_flag_combinations_respect_disabled_kinds() {
    let f = fixture_v2();
    let key = f.source.key.clone();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    committer
        .commit(
            "flag-entities",
            vec![
                v2_entity_assertion(
                    "fe_c2",
                    "c2",
                    EntityKindV2::Concept,
                    "Second Concept",
                    &[],
                    None,
                    &f.source,
                    PROJECT,
                    f.now_ms,
                ),
                v2_entity_assertion(
                    "fe_m2",
                    "m2",
                    EntityKindV2::Metric,
                    "Other Latency",
                    &[],
                    None,
                    &f.source,
                    PROJECT,
                    f.now_ms,
                ),
            ],
        )
        .unwrap();
    let depends =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| id.to_string()).collect() };
    committer
        .commit(
            "flag-edges",
            vec![
                v2_relation_assertion(
                    "edge_corr",
                    v2_relation_value(
                        "m1",
                        "m2",
                        "correlates_with",
                        None,
                        &[],
                        None,
                        None,
                        Some("negative"),
                        None,
                        std::slice::from_ref(&key),
                        None,
                        None,
                    ),
                    &f.source,
                    PROJECT,
                    depends(&["ent_m1", "fe_m2"]),
                    f.now_ms,
                ),
                v2_relation_assertion(
                    "edge_dep",
                    v2_relation_value(
                        "c1",
                        "c2",
                        "depends_on",
                        None,
                        &[],
                        None,
                        None,
                        None,
                        None,
                        std::slice::from_ref(&key),
                        None,
                        None,
                    ),
                    &f.source,
                    PROJECT,
                    depends(&["ent_c1", "fe_c2"]),
                    f.now_ms,
                ),
            ],
        )
        .unwrap();
    for mask in 0..16u8 {
        let flags = IncludeFlags {
            causal: mask & 1 != 0,
            goals: mask & 2 != 0,
            correlations: mask & 4 != 0,
            dependencies: mask & 8 != 0,
        };
        let slice = run_v2(
            &f.writer,
            &[WorldSeed::EntityId("c1".into())],
            flags,
            8_192,
            now(),
        )
        .unwrap();
        if !flags.causal {
            assert!(
                !slice.relations.iter().any(|relation| matches!(
                    relation.relation_type,
                    saaa_personal_state_core::world::model_v2::RelationTypeV2::Increases
                        | saaa_personal_state_core::world::model_v2::RelationTypeV2::Decreases
                        | saaa_personal_state_core::world::model_v2::RelationTypeV2::Causes
                        | saaa_personal_state_core::world::model_v2::RelationTypeV2::Enables
                        | saaa_personal_state_core::world::model_v2::RelationTypeV2::Inhibits
                )),
                "causal disabled but a signed-effect relation was returned (mask {mask})"
            );
        }
        if !flags.goals {
            assert!(
                !slice
                    .nodes
                    .iter()
                    .any(|node| node.entity_kind == EntityKindV2::Goal),
                "goals disabled but a Goal node was returned (mask {mask})"
            );
            assert!(
                !slice.relations.iter().any(|relation| matches!(
                    relation.relation_type,
                    saaa_personal_state_core::world::model_v2::RelationTypeV2::HasGoal
                        | saaa_personal_state_core::world::model_v2::RelationTypeV2::ServesGoal
                )),
                "goals disabled but a goal relation was returned (mask {mask})"
            );
        }
        if !flags.correlations {
            assert!(
                !slice.relations.iter().any(|relation| relation.relation_type
                    == saaa_personal_state_core::world::model_v2::RelationTypeV2::CorrelatesWith),
                "correlations disabled but a correlation was returned (mask {mask})"
            );
        }
        if !flags.dependencies {
            assert!(
                !slice.relations.iter().any(|relation| relation.relation_type
                    == saaa_personal_state_core::world::model_v2::RelationTypeV2::DependsOn),
                "dependencies disabled but a dependency was returned (mask {mask})"
            );
        }
    }
}

#[test]
fn d17_swapped_v2_payload_content_is_rejected_on_replay() {
    let f = fixture_v2();
    let (prediction, outcome) = counterexample(
        &f.source,
        EffectDirection::Increase,
        EffectDirection::Decrease,
    );
    let mut prepared = f
        .writer
        .read_serialized(|c| {
            let ledger = store::load(c)?;
            prepare_outcome_patch(c, &ledger, PROJECT, "rel_inc", &prediction, &outcome, now())
        })
        .unwrap();
    // Swap the outer payload for one with different content but the same ref.
    let (reference, value) = prepared
        .payloads
        .iter()
        .next()
        .map(|(key, value)| (key.clone(), value.clone()))
        .unwrap();
    let mut tampered = value.clone();
    tampered["confidence"]["value"] = json!(641);
    assert_ne!(tampered, value);
    prepared.payloads.insert(reference, tampered);
    let error = commit_prepared_outcome(&f.writer, &prepared, "fence-tamper").unwrap_err();
    assert_eq!(error, "world-invalid-payload");
}

#[test]
fn d21_unauthorized_access_is_an_error_not_an_empty_slice() {
    use super::query_v2::ActivateInputV2 as InputV2;
    let f = fixture_v2();
    let result = f.writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let mut request = access(&ledger, PROJECT);
        request.authorized = false;
        activate_v2(
            c,
            &InputV2 {
                project_scope: PROJECT,
                access: &request,
                now: now(),
                seeds: &[WorldSeed::EntityId("c1".into())],
                causal_direction: CausalDirection::Forward,
                limits: LimitsV2::m1(),
                max_bytes: 8_192,
                request_id: "req-denied",
                explicit_question: false,
                flags: IncludeFlags::default(),
                condition_observations: &[],
                availability_observations: &[],
                temporary_attention_entity_ids: &[],
            },
        )
    });
    assert_eq!(result.unwrap_err(), "world-scope-denied");
}

#[test]
fn d21_ambiguous_name_is_reported_as_a_candidate_not_resolved() {
    let f = fixture_v2();
    let mut committer = Committer {
        writer: &f.writer,
        project: PROJECT,
    };
    // Second entity with the same display name in the same project.
    committer
        .commit(
            "ambiguous",
            vec![v2_entity_assertion(
                "amb1",
                "amb1",
                EntityKindV2::Concept,
                "Speculative Decoding",
                &[],
                None,
                &f.source,
                PROJECT,
                f.now_ms,
            )],
        )
        .unwrap();
    let slice = run_v2(
        &f.writer,
        &[WorldSeed::ExactName("Speculative Decoding".into())],
        IncludeFlags::default(),
        8_192,
        now(),
    )
    .unwrap();
    assert!(slice.nodes.is_empty());
    assert!(slice
        .notices
        .iter()
        .any(|notice| notice == "ambiguous_seed"));
}

#[test]
fn d22_observation_from_another_project_is_dropped_with_a_notice() {
    use super::observations_v2::ConditionObservationInput;
    let f = fixture_v2();
    let other = f
        .writer
        .write(|c| {
            Ok(insert_source(
                c,
                "project:other",
                "other-obs",
                "other source",
            ))
        })
        .unwrap();
    // Adding a source advances the epoch, so refresh the projection before the read.
    f.writer.write(|c| store::rebuild(c, now())).unwrap();
    let observation = ConditionObservationInput {
        relation_assertion_id: "rel_inc".into(),
        key: "config".into(),
        value: "a".into(),
        source: other.key.clone(),
    };
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
                    seeds: &[WorldSeed::EntityId("c1".into())],
                    causal_direction: CausalDirection::Forward,
                    limits: LimitsV2::m1(),
                    max_bytes: 8_192,
                    request_id: "req-obs",
                    explicit_question: false,
                    flags: IncludeFlags::default(),
                    condition_observations: std::slice::from_ref(&observation),
                    availability_observations: &[],
                    temporary_attention_entity_ids: &[],
                },
            )
        })
        .unwrap();
    assert!(slice
        .notices
        .iter()
        .any(|notice| notice == "observation_unavailable"));
}

#[test]
fn d22_more_than_thirty_observations_is_a_limit() {
    use super::observations_v2::{ConditionObservationInput, MAX_OBSERVATIONS};
    let f = fixture_v2();
    let observations: Vec<ConditionObservationInput> = (0..=MAX_OBSERVATIONS)
        .map(|index| ConditionObservationInput {
            relation_assertion_id: "rel_inc".into(),
            key: format!("k{index}"),
            value: "a".into(),
            source: f.source.key.clone(),
        })
        .collect();
    let result = f.writer.read_serialized(|c| {
        let ledger = store::load(c)?;
        let request = access(&ledger, PROJECT);
        activate_v2(
            c,
            &ActivateInputV2 {
                project_scope: PROJECT,
                access: &request,
                now: now(),
                seeds: &[WorldSeed::EntityId("c1".into())],
                causal_direction: CausalDirection::Forward,
                limits: LimitsV2::m1(),
                max_bytes: 8_192,
                request_id: "req-limit",
                explicit_question: false,
                flags: IncludeFlags::default(),
                condition_observations: &observations,
                availability_observations: &[],
                temporary_attention_entity_ids: &[],
            },
        )
    });
    assert_eq!(result.unwrap_err(), "world-limit");
}

#[test]
fn d19_failed_commit_rolls_back_every_write() {
    use saaa_personal_state_core::world::EntityKind;
    use std::collections::BTreeMap;
    let writer = writer_db();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "rb1", "rollback source")))
        .unwrap();
    // Commit clock must not precede the source's recorded_at.
    let now_ms = now();
    let revision_before: u64 = writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT revision FROM personal_scope WHERE id='primary'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    let built = vec![
        entity_assertion(
            "rb_e1",
            "rb_e1p",
            "rbe1",
            EntityKind::Concept,
            "RB1",
            &[],
            &source,
            PROJECT,
            now_ms,
        ),
        entity_assertion(
            "rb_e2",
            "rb_e2p",
            "rbe2",
            EntityKind::Concept,
            "RB2",
            &[],
            &source,
            PROJECT,
            now_ms,
        ),
    ];
    // A payload byte count that does not match is a late failure after the
    // first assertion has already been written; the transaction must undo it.
    let error = writer
        .write(|c| {
            let ledger = store::load(c)?;
            let patch_id = crate::new_id("patch");
            let mut assertions = Vec::new();
            let mut payload_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for item in built {
                payload_map.insert(item.assertion.payload_ref.clone(), item.payload);
                assertions.push(item.assertion);
            }
            for assertion in assertions.iter_mut() {
                assertion.recorded_at = now_ms;
                assertion.effective_at = now_ms;
            }
            let inputs: BTreeSet<SourceKey> = assertions
                .iter()
                .flat_map(|a| a.input_dependencies.iter().cloned())
                .collect();
            let mut transitions = Vec::new();
            let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            for assertion in &assertions {
                transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence,
                    assertion_id: assertion.id.clone(),
                    action: Action::Assert,
                    reason_code: "world-fixture".into(),
                    evidence: assertion.evidence.clone(),
                    input_dependencies: assertion.input_dependencies.clone(),
                    recorded_at: now_ms,
                });
                sequence += 1;
            }
            let patch = StatePatch {
                id: patch_id.clone(),
                base_revision: ledger.revision,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                fence: "fence-rollback".into(),
                assertions,
                transitions,
                coverage: Vec::new(),
            };
            let mut payload_bytes: BTreeMap<String, usize> = payload_map
                .iter()
                .map(|(k, v)| Ok((k.clone(), crate::memory::personal_state::encode(v)?.len())))
                .collect::<Result<_, String>>()?;
            payload_bytes.insert("rb_e2p".into(), 1);
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: Some(PROJECT),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: "fence-rollback",
                issued_patch_id: &patch_id,
                issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
                issued_payload_bytes: payload_bytes,
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &payload_map)
        })
        .unwrap_err();
    assert_eq!(
        error, "personal-payload-budget",
        "unexpected error: {error}"
    );
    writer
        .read_serialized(|c| {
            let assertions: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_assertions WHERE id IN ('rb_e1','rb_e2')",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let payloads: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_payloads WHERE id IN ('rb_e1p','rb_e2p')",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            let revision: u64 = c
                .query_row(
                    "SELECT revision FROM personal_scope WHERE id='primary'",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)?;
            assert_eq!(assertions, 0, "assertions must roll back");
            assert_eq!(payloads, 0, "payloads must roll back");
            assert_eq!(revision, revision_before, "revision must roll back");
            Ok(())
        })
        .unwrap();
}

#[test]
fn d13_v1_relation_can_be_superseded_by_an_equivalent_v2_relation() {
    use saaa_personal_state_core::world::{
        Basis as V1Basis, EffectInput as V1Effect, EntityKind as V1Kind, RelationType as V1Type,
    };
    let writer = writer_db();
    let source = writer
        .write(|c| Ok(insert_source(c, PROJECT, "xv", "cross-version source")))
        .unwrap();
    let now_ms = now();
    let mut committer = Committer {
        writer: &writer,
        project: PROJECT,
    };
    committer
        .commit(
            "xv-base",
            vec![
                entity_assertion(
                    "xv_c1",
                    "xv_c1p",
                    "xvc1",
                    V1Kind::Concept,
                    "XV Concept",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
                entity_assertion(
                    "xv_m1",
                    "xv_m1p",
                    "xvm1",
                    V1Kind::Metric,
                    "XV Metric",
                    &[],
                    &source,
                    PROJECT,
                    now_ms,
                ),
            ],
        )
        .unwrap();
    committer
        .commit(
            "xv-v1",
            vec![relation_assertion(
                "xv_v1",
                "xv_v1p",
                "xvc1",
                "xvm1",
                V1Type::Increases,
                Some(V1Effect::Intervention),
                &[],
                V1Basis::ModelHypothesis,
                vec![(
                    source.key.clone(),
                    saaa_personal_state_core::world::Stance::Context,
                )],
                &source,
                PROJECT,
                BTreeSet::from(["xv_c1".to_string(), "xv_m1".to_string()]),
                now_ms,
            )],
        )
        .unwrap();
    // Same logical identity (same valid_from), v2 payload, explicit Supersede.
    let v2 = v2_relation_assertion(
        "xv_v2",
        v2_relation_value(
            "xvc1",
            "xvm1",
            "increases",
            Some("intervention"),
            &[],
            None,
            None,
            None,
            None,
            std::slice::from_ref(&source.key),
            None,
            None,
        ),
        &source,
        PROJECT,
        BTreeSet::from(["xv_c1".to_string(), "xv_m1".to_string()]),
        now_ms,
    );
    committer
        .commit_superseding("xv-supersede", vec![v2], "xv_v1")
        .unwrap();
    // The v1 assertion is superseded and the v2 one is live; a v2 query reads it.
    let slice = run_v2(
        &writer,
        &[WorldSeed::EntityId("xvc1".into())],
        IncludeFlags::default(),
        8_192,
        now(),
    )
    .unwrap();
    assert!(slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "xv_v2"));
    assert!(!slice
        .relations
        .iter()
        .any(|relation| relation.assertion_id == "xv_v1"));
}
