use super::*;
#[test]
pub(super) fn world_g1_04_graph_only_question_fetches_a_frame_without_runtime_refs() {
    let fixture = g1_fixture();
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        fixture.access().principal,
        fixture.access().policy_revision,
        Some(graph_request("Speculative Decoding")),
        RUN_ID,
        &scope,
        window(),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    let candidate = world_candidate(&composed).expect("world candidate");
    // The combined block is exactly what the broker inserts before the current instruction and the
    // provider history/body is rendered from; the graph JSON must survive verbatim.
    let combined = composed
        .envelope
        .combined_block
        .as_deref()
        .expect("combined block");
    assert!(combined.contains(&candidate.content));
    assert!(combined.contains("\"nodes\""));
    assert!(combined.contains("\"notices\""));
    let json = parse_rendered_json(&candidate.content);
    assert!(json["runtime"].as_array().unwrap().is_empty());
    let names = json["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"Speculative Decoding"),
        "names={names:?} json={json}"
    );
    assert!(
        names.contains(&"Decode Latency"),
        "names={names:?} json={json}"
    );
    assert!(
        names.contains(&"Voice Latency"),
        "names={names:?} json={json}"
    );
    let relation_types = json["graph"]["relations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|relation| relation["relation_type"].as_str())
        .collect::<Vec<_>>();
    assert!(
        relation_types.contains(&"correlates_with"),
        "{relation_types:?}"
    );
    assert!(relation_types.contains(&"depends_on"), "{relation_types:?}");
}
#[test]
pub(super) fn world_g1_06_alias_resolves_only_when_it_is_unique() {
    let fixture = g1_fixture();
    let outcome = prepare_outcome(&fixture, graph_request("spec-dec"), true);
    let ready = match outcome {
        WorldSourceOutcome::Ready(ready) => ready,
        other => panic!(
            "unique alias must resolve, got omission {:?}",
            omission_label(&other)
        ),
    };
    let json = parse_rendered_json(&ready.candidate().content);
    let names = json["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Speculative Decoding"), "{names:?}");
}
#[test]
pub(super) fn world_g1_07_plain_chat_keeps_the_runtime_only_path() {
    let fixture = g1_fixture();
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        fixture.access().principal,
        fixture.access().policy_revision,
        None,
        RUN_ID,
        &scope,
        window(),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    assert!(world_candidate(&composed).is_none());
    assert!(composed.world.is_none());
}
#[test]
pub(super) fn world_g1_01_not_requested_never_builds_a_graph_request() {
    assert!(parse_graph_question(TECH_QUESTION).is_requested());
    assert_eq!(
        parse_graph_question("普通の雑談です"),
        QuestionParse::NotRequested
    );
    assert_eq!(
        parse_graph_question(TECH_QUESTION)
            .graph_request()
            .unwrap()
            .seeds,
        vec![WorldSeed::ExactName("Speculative Decoding".into())]
    );
    assert_eq!(
        graph_request("Speculative Decoding").limits,
        LimitsV2::m1().capped()
    );
    assert_eq!(
        graph_request("Speculative Decoding").causal_direction,
        CausalDirection::Forward
    );
    assert_eq!(
        graph_request("Speculative Decoding").flags,
        IncludeFlags::default()
    );
}
#[test]
pub(super) fn world_g1_saved_input_reaches_the_app_composer() {
    let fixture = g1_fixture_input(Some(TECH_QUESTION));
    fixture.writer.write(|c| {
        c.execute("UPDATE runtime_run_scopes SET epoch=(SELECT epoch FROM context_scope_epochs e WHERE e.scope_key=runtime_run_scopes.scope_key)", []).map_err(crate::database_error)?;
        Ok(())
    }).unwrap();
    let capabilities = Arc::new(
        crate::generated_capabilities::service::CapabilityService::build(
            fixture.writer.clone(),
            &std::path::PathBuf::new(),
            std::path::PathBuf::new(),
            None,
        ),
    );
    let state =
        crate::test_state::app_state_with_capabilities(fixture.writer.clone(), capabilities);
    let scope = load_scope(&fixture);
    let composed = super::super::turn::compose_for_app_enabled(
        &state,
        RUN_ID,
        &scope,
        window(),
        vec![],
        allowed(&scope),
        true,
    )
    .unwrap();
    let candidate = world_candidate(&composed).expect("saved explicit question produces graph");
    assert!(
        parse_rendered_json(&candidate.content)["graph"]["nodes"]
            .as_array()
            .is_some_and(|n| !n.is_empty()),
        "{}",
        candidate.content
    );
}
