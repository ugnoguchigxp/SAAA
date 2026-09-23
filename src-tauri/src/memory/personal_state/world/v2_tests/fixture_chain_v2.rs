use super::*;
pub(super) fn fixture_chain_v2() -> ChainFixture {
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
pub(super) fn d25_reverse_causal_search_composes_in_declared_order() {
    use super::super::observations_v2::ConditionObservationInput;
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
pub(super) fn d32_all_flag_combinations_respect_disabled_kinds() {
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
pub(super) fn d17_swapped_v2_payload_content_is_rejected_on_replay() {
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
pub(super) fn d21_unauthorized_access_is_an_error_not_an_empty_slice() {
    use super::super::query_v2::ActivateInputV2 as InputV2;
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
pub(super) fn d21_ambiguous_name_is_reported_as_a_candidate_not_resolved() {
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
pub(super) fn d22_observation_from_another_project_is_dropped_with_a_notice() {
    use super::super::observations_v2::ConditionObservationInput;
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
pub(super) fn d22_more_than_thirty_observations_is_a_limit() {
    use super::super::observations_v2::{ConditionObservationInput, MAX_OBSERVATIONS};
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
