//! WM-10..WM-13 pure traversal / relevance contract tests.
use saaa_personal_state_core::world::relevance::{build_gaps, gap_key, trim_to_budget, GapInput};
use saaa_personal_state_core::world::traversal::{
    maximal_only, traverse, CausalDirection, EdgeStatus, Limits, TraversalMode, WorldEdge,
    MAX_DEPTH, MAX_EDGES, MAX_NODES, MAX_PATHS, MAX_SCAN,
};
use saaa_personal_state_core::world::{EntityKind, ResearchGap, SliceFocus, SliceNode, WorldSlice};

fn edge(
    id: &str,
    from: &str,
    to: &str,
    rt: &str,
    status: EdgeStatus,
    conditions: &[(&str, &str)],
) -> WorldEdge {
    WorldEdge {
        assertion_id: id.into(),
        semantic_key: format!("wm1:{id}"),
        from: from.into(),
        to: to.into(),
        relation_type: rt.into(),
        status,
        conditions: conditions
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        basis: "model_hypothesis".into(),
    }
}

fn related_edges() -> Vec<WorldEdge> {
    vec![
        edge(
            "r1",
            "e1",
            "e2",
            "decreases",
            EdgeStatus::Active,
            &[("config", "a")],
        ),
        edge(
            "r2",
            "e2",
            "e3",
            "increases",
            EdgeStatus::Active,
            &[("config", "a")],
        ),
        edge("r3", "e3", "e4", "important_for", EdgeStatus::Active, &[]),
    ]
}

#[test]
fn t16_forward_reverse_diamond_cycle() {
    let edges = related_edges();
    let forward = traverse(
        &edges,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    // Related traversal preserves the declared direction on each step.
    let full = forward
        .paths
        .iter()
        .find(|p| p.nodes == vec!["e1", "e2", "e3", "e4"])
        .expect("full path");
    assert!(full.steps.iter().all(|s| !s.traversed_reverse));
    // Reverse causal search keeps the arrow's original meaning.
    let reverse = traverse(
        &edges,
        &["e3".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Reverse,
    );
    let step = &reverse.paths[0].steps[0];
    assert!(step.traversed_reverse);
    assert_eq!(edges[step.edge].from, "e2");
    assert_eq!(edges[step.edge].to, "e3");

    // Diamond: two distinct paths to the same node are both retained.
    let diamond = vec![
        edge("a", "s", "x", "related_to", EdgeStatus::Active, &[]),
        edge("b", "s", "y", "related_to", EdgeStatus::Active, &[]),
        edge("c", "x", "z", "related_to", EdgeStatus::Active, &[]),
        edge("d", "y", "z", "related_to", EdgeStatus::Active, &[]),
    ];
    let outcome = traverse(
        &diamond,
        &["s".into()],
        Limits::default(),
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    assert!(outcome.paths.iter().any(|p| p.nodes == vec!["s", "x", "z"]));
    assert!(outcome.paths.iter().any(|p| p.nodes == vec!["s", "y", "z"]));

    // A single path never revisits an entity even when edges form a cycle.
    let cycle = vec![
        edge("c1", "e1", "e2", "related_to", EdgeStatus::Active, &[]),
        edge("c2", "e2", "e1", "related_to", EdgeStatus::Active, &[]),
    ];
    let outcome = traverse(
        &cycle,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    for path in &outcome.paths {
        let unique: std::collections::BTreeSet<_> = path.nodes.iter().collect();
        assert_eq!(unique.len(), path.nodes.len());
    }
}

#[test]
fn t17_each_limit_is_enforced_with_a_reason() {
    let mut edges = Vec::new();
    for index in 0..20 {
        edges.push(edge(
            &format!("r{index}"),
            "s",
            &format!("n{index}"),
            "related_to",
            EdgeStatus::Active,
            &[],
        ));
    }
    let nodes = traverse(
        &edges,
        &["s".into()],
        Limits {
            nodes: 3,
            ..Limits::default()
        },
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    assert!(nodes.truncated);
    assert_eq!(nodes.truncation_reason, Some("nodes"));
    let scan = traverse(
        &edges,
        &["s".into()],
        Limits {
            scan: 2,
            ..Limits::default()
        },
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    assert_eq!(scan.truncation_reason, Some("scan"));
}

#[test]
fn t18_t19_causal_excludes_non_causal_and_enforces_conditions() {
    let edges = related_edges();
    let causal = traverse(
        &edges,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Forward,
    );
    // important_for must never enter a causal path.
    assert!(causal.paths.iter().all(|p| p
        .steps
        .iter()
        .all(|s| edges[s.edge].relation_type != "important_for")));
    let maximal = maximal_only(&causal.paths);
    assert_eq!(maximal.len(), 1);
    assert_eq!(maximal[0].nodes, vec!["e1", "e2", "e3"]);
    // Disputed edges are excluded from causal traversal.
    let mut disputed = edges.clone();
    disputed[0].status = EdgeStatus::Disputed;
    let causal = traverse(
        &disputed,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Forward,
    );
    assert!(causal.paths.is_empty());
    // A path touching an empty-condition edge is flagged as unverified.
    let empty = vec![
        edge("a", "e1", "e2", "increases", EdgeStatus::Active, &[]),
        edge(
            "b",
            "e2",
            "e3",
            "increases",
            EdgeStatus::Active,
            &[("config", "a")],
        ),
    ];
    let outcome = traverse(
        &empty,
        &["e1".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Forward,
    );
    let path = outcome
        .paths
        .iter()
        .find(|p| p.nodes == vec!["e1", "e2", "e3"])
        .expect("path");
    assert!(path.conditions_unverified);
}

#[test]
fn t21_gap_keys_are_stable_and_slice_budget_is_enforced() {
    let a = gap_key("project:x", "wm1:r1", "e4", "hypothesis_unverified");
    let b = gap_key("project:x", "wm1:r1", "e4", "hypothesis_unverified");
    assert_eq!(a, b);
    let c = gap_key("project:x", "wm1:r1", "e4", "disputed");
    assert_ne!(a, c);
    let mut slice = WorldSlice::empty(1, 1);
    slice.nodes.push(SliceNode {
        entity_id: "e4".into(),
        name: "Speculative Decodingの導入".into(),
        entity_kind: EntityKind::Metric,
    });
    for index in 0..120 {
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
    assert_eq!(
        trim_to_budget(WorldSlice::empty(1, 1), 32)
            .unwrap_err()
            .code(),
        "world-budget-too-small"
    );
}

#[test]
fn r04_causal_reverse_does_not_follow_forward_edges() {
    let edges = vec![edge("r1", "a", "b", "increases", EdgeStatus::Active, &[])];
    let reverse_from_source = traverse(
        &edges,
        &["a".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Reverse,
    );
    assert!(
        reverse_from_source.paths.is_empty(),
        "a reverse search from a cause must not return its effect: {:?}",
        reverse_from_source.paths
    );
    let forward = traverse(
        &edges,
        &["a".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Forward,
    );
    assert_eq!(forward.paths[0].nodes, vec!["a", "b"]);
    let reverse = traverse(
        &edges,
        &["b".into()],
        Limits::default(),
        TraversalMode::Causal,
        CausalDirection::Reverse,
    );
    assert_eq!(reverse.paths[0].nodes, vec!["b", "a"]);
    assert!(reverse.paths[0].steps[0].traversed_reverse);
}

#[test]
fn r10_prefix_paths_do_not_consume_the_return_cap() {
    let mut edges = Vec::new();
    for index in 0..10 {
        edges.push(edge(
            &format!("r{index}"),
            "s",
            &format!("m{index}"),
            "increases",
            EdgeStatus::Active,
            &[("config", "a")],
        ));
    }
    edges.push(edge(
        "goal",
        "m0",
        "g",
        "increases",
        EdgeStatus::Active,
        &[("config", "a")],
    ));
    let limits = Limits {
        paths: 10,
        depth: 3,
        ..Limits::default()
    };
    let outcome = traverse(
        &edges,
        &["s".into()],
        limits,
        TraversalMode::Causal,
        CausalDirection::Forward,
    );
    let maximal = maximal_only(&outcome.paths);
    assert!(
        maximal.iter().any(|p| p.nodes == vec!["s", "m0", "g"]),
        "the two-hop path must survive ten prefix paths: {:?}",
        outcome.paths
    );
}

#[test]
fn r09_trim_drops_focus_and_gaps_whose_node_is_absent() {
    let mut slice = WorldSlice::empty(1, 1);
    slice.focus.push(SliceFocus {
        entity_id: "ghost".into(),
        reason: "explicit_interest".into(),
        objective_assertion_id: None,
    });
    slice.research_gaps.push(ResearchGap {
        key: "k".into(),
        reason: "hypothesis_unverified".into(),
        relation_assertion_id: "r".into(),
        focus_entity_id: "ghost".into(),
        question: "q".into(),
    });
    let trimmed = trim_to_budget(slice, 8_192).unwrap();
    assert!(
        trimmed.focus.is_empty(),
        "orphan focus must not be returned"
    );
    assert!(
        trimmed.research_gaps.is_empty(),
        "gaps for an absent node must not be returned"
    );
}

#[test]
fn r11_gaps_order_by_reason_then_focus_rank_then_path_length() {
    let disputed = edge(
        "rel-disputed",
        "e1",
        "e3",
        "increases",
        EdgeStatus::Disputed,
        &[("config", "a")],
    );
    let hypothesis = edge(
        "rel-hyp",
        "e1",
        "e2",
        "increases",
        EdgeStatus::Active,
        &[("config", "a")],
    );
    let names: std::collections::BTreeMap<String, String> = [
        ("e2".to_string(), "E2".to_string()),
        ("e3".to_string(), "E3".to_string()),
    ]
    .into_iter()
    .collect();
    // Reason priority dominates: a disputed edge outranks a plain hypothesis even
    // with a worse Focus rank and a longer path.
    let inputs = vec![
        GapInput {
            focus_entity_id: "e2".to_string(),
            focus_rank: 0,
            path_len: 1,
            edge: &hypothesis,
        },
        GapInput {
            focus_entity_id: "e3".to_string(),
            focus_rank: 1,
            path_len: 5,
            edge: &disputed,
        },
    ];
    let gaps = build_gaps("project:x", &names, &inputs);
    assert_eq!(gaps[0].relation_assertion_id, "rel-disputed");
    // With equal reason, the better Focus rank and shorter path win.
    let actual = edge(
        "rel-a",
        "e1",
        "e3",
        "increases",
        EdgeStatus::Active,
        &[("config", "a")],
    );
    let inputs = vec![
        GapInput {
            focus_entity_id: "e3".to_string(),
            focus_rank: 1,
            path_len: 1,
            edge: &actual,
        },
        GapInput {
            focus_entity_id: "e2".to_string(),
            focus_rank: 0,
            path_len: 3,
            edge: &hypothesis,
        },
    ];
    let gaps = build_gaps("project:x", &names, &inputs);
    assert_eq!(gaps[0].focus_entity_id, "e2");
    assert_eq!(gaps[1].focus_entity_id, "e3");
}

#[test]
fn r12_limits_are_capped_to_the_fixed_maxima() {
    let capped = Limits {
        depth: 99,
        nodes: 9_999,
        edges: 9_999,
        paths: 9_999,
        scan: 999_999,
    }
    .capped();
    assert_eq!(capped.depth, MAX_DEPTH);
    assert_eq!(capped.nodes, MAX_NODES);
    assert_eq!(capped.edges, MAX_EDGES);
    assert_eq!(capped.paths, MAX_PATHS);
    assert_eq!(capped.scan, MAX_SCAN);
    // Smaller requests are preserved so a caller can always shrink the budget.
    let smaller = Limits {
        depth: 1,
        nodes: 2,
        edges: 3,
        paths: 4,
        scan: 5,
    }
    .capped();
    assert_eq!(
        (
            smaller.depth,
            smaller.nodes,
            smaller.edges,
            smaller.paths,
            smaller.scan
        ),
        (1, 2, 3, 4, 5)
    );
}
