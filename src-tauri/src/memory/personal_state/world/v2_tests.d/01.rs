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
