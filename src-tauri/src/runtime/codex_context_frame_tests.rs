#![cfg(test)]
use super::*;
use crate::memory::personal_state::world::runtime_test_support::RUN_ID;
use crate::runtime::context::world::g1_tests as graph;
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
    let prompt = dispatch.prepare().unwrap();
    assert!(prompt.contains("correlates_with"));
    assert!(prompt.contains("situation"));
    let thread = json!({"params":{"developerInstructions":prompt}});
    let turn = json!({"params":{"input":[{"type":"text","text":"hello"}]}});
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
