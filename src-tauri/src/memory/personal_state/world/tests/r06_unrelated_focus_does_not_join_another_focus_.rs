use super::*;
#[test]
pub(super) fn r06_unrelated_focus_does_not_join_another_focus_gap() {
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
pub(super) fn r07_nodes_are_bounded_even_with_focus() {
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
pub(super) fn r08_small_byte_budget_is_an_explicit_error() {
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
pub(super) fn r11_causal_project_start_is_rejected() {
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
pub(super) fn r12_seed_set_must_be_bounded_and_fully_resolved() {
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
pub(super) fn r13_candidate_endpoint_cannot_be_referenced() {
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
