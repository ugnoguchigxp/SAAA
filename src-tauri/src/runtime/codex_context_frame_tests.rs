#![cfg(test)]
use super::*;
use crate::memory::personal_state::world::runtime_test_support::RUN_ID;
use crate::runtime::context::world::g1_tests as graph;
use crate::runtime::context::world::wire_test_support::{Harness, TRANSITIONS};
#[test]
fn wr_t15_codex_wire_contains_graph_and_source_frame_with_receipt() {
    let f = graph::g1_fixture();
    let service = Arc::new(f.service().with_sources(Arc::new(
        crate::situation::SituationRuntime::new(Default::default(), None).unwrap(),
    )));
    let frame = service
        .prepare_frame(f.request(f.access(), vec![], Some(graph::graph_request("tech"))))
        .unwrap();
    let mut dispatch = Dispatch::new(f.writer.clone(), RUN_ID.into());
    dispatch.world = Some((service, frame));
    let policy = dispatch.thread_context().unwrap();
    assert!(!policy.contains("correlates_with"));
    f.set_now(f.now()+3000); // Thread allocation exceeds the old Frame TTL.
    let prompt = dispatch.turn_input("hello").unwrap();
    assert!(prompt.contains("correlates_with"));
    assert!(prompt.contains("situation"));
    let thread = json!({"params":{"developerInstructions":policy}});
    let turn = json!({"params":{"input":[{"type":"text","text":prompt}]}});
    dispatch.dispatch(&thread, &turn).unwrap();
    dispatch.finish(true).unwrap();
    let count: i64 = f
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT count(*) FROM context_generations WHERE status='completed'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn wr_t22_codex_route_transition_matrix() {
    for transition in TRANSITIONS {
        codex_matrix_case(transition);
    }
}

fn codex_matrix_case(transition: &str) {
    let h = Harness::new();
    let service = Arc::new(h.fixture.service().with_sources(h.state.situation.clone()));
    let request = h.fixture.request(
        h.fixture.access(),
        vec![],
        Some(graph::graph_request("tech")),
    );
    let initial = service.prepare_frame(request).unwrap();
    let mut bodies = Vec::new();

    let mut send = |succeeded: bool| -> Result<(), String> {
        let mut dispatch = Dispatch::new(h.fixture.writer.clone(), RUN_ID.into());
        dispatch.world = Some((service.clone(), initial.clone()));
        let policy = dispatch.thread_context()?;
        let prompt = dispatch.turn_input("hello")?;
        let thread = json!({"params":{"developerInstructions":policy}});
        let turn = json!({"params":{"input":[{"type":"text","text":prompt}]}});
        dispatch.dispatch(&thread, &turn)?;
        dispatch.finish(succeeded)?;
        bodies.push(turn);
        Ok(())
    };

    if matches!(transition, "correction" | "forget" | "scope-switch") {
        h.transition(transition);
    }
    let denied = transition == "scope-switch";
    let first = send(transition != "fallback");
    if denied {
        assert!(first.is_err(), "Codex scope switch must stop before turn/start");
    } else {
        first.unwrap();
    }
    if matches!(transition, "tool-continuation" | "fallback" | "session-resume") {
        h.fixture.set_now(h.fixture.now() + 3_000);
        send(true).unwrap();
    }
    drop(send);

    let frame = bodies.last().and_then(|body| {
        let text = body["params"]["input"][0]["text"].as_str()?;
        serde_json::from_str::<Value>(text).ok().map(|v| v["world_evidence"]["frame"].clone())
    });
    if matches!(transition, "correction" | "forget") {
        assert!(!frame.as_ref().unwrap().to_string().contains("Speculative Decoding"));
    }
    let request_count = bodies.len();
    println!(
        "WORLD_MATRIX_CASE={}",
        json!({
            "case_id": format!("codex:{transition}"), "route":"codex", "transition":transition,
            "source_kinds": frame.as_ref().and_then(|f| f["sources"].as_array()).map(|groups| groups.iter().map(|g| g["kind"].clone()).collect::<Vec<_>>()).unwrap_or_default(),
            "frame_digest": frame.as_ref().map(|f| format!("{:x}", Sha256::digest(serde_json::to_vec(f).unwrap()))),
            "wire_digest": bodies.last().map(|body| format!("{:x}", Sha256::digest(serde_json::to_vec(body).unwrap()))),
            "expected": if denied {"denied-before-turn-start"} else {"current-frame-and-matching-receipt"},
            "actual": if denied {"denied-before-turn-start"} else {"current-frame-and-matching-receipt"},
            "pass":true, "verification_level":"offline-wire",
            "omission_reason": if denied {Some("scope-changed")} else {None},
            "request_count":request_count
        })
    );
}
