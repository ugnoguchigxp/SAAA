#![cfg(test)]
//! Real-clock performance; unlike lifetime tests this never measures a fake clock.
use super::g1_tests::{g1_fixture, graph_request};
use std::time::{Duration, Instant};

#[test]
#[ignore = "performance gate: run explicitly on a quiet development host"]
fn world_g1_performance() {
    let f = g1_fixture();
    use crate::memory::personal_state::world::test_support::{
        insert_source, v2_entity_assertion, Committer, PROJECT,
    };
    use saaa_personal_state_core::world::model_v2::EntityKindV2;
    let source = f
        .writer
        .write(|c| {
            Ok(insert_source(
                c,
                PROJECT,
                "perf-extra",
                "synthetic performance",
            ))
        })
        .unwrap();
    let initial: usize = f.writer.read_serialized(|c| c.query_row("SELECT (SELECT COUNT(*) FROM personal_world_entities)+(SELECT COUNT(*) FROM personal_world_relations)+(SELECT COUNT(*) FROM personal_world_focus)", [], |r| r.get(0)).map_err(crate::database_error)).unwrap();
    let mut committer = Committer {
        writer: f.writer.as_ref(),
        project: PROJECT,
    };
    for i in initial..100 {
        let id = format!("perf-{i}");
        committer
            .commit(
                &id,
                vec![v2_entity_assertion(
                    &id,
                    &id,
                    EntityKindV2::Concept,
                    &id,
                    &[],
                    None,
                    &source,
                    PROJECT,
                    f.now(),
                )],
            )
            .unwrap();
    }
    f.writer
        .write(|c| {
            c.execute("UPDATE personal_jobs SET status='completed'", [])
                .map(|_| ())
                .map_err(crate::database_error)
        })
        .unwrap();
    let ledger = f.ledger_count();
    assert!(ledger <= 2000);
    f.fill_coverage((2000 - ledger) as usize);
    assert_eq!(f.ledger_count(), 2000);
    let service = crate::memory::personal_state::world::runtime_frame::WorldFrameService::new(
        f.readers(),
        std::sync::Arc::new(crate::memory::personal_state::now),
    );
    let mut prepare = Vec::new();
    let mut validate = Vec::new();
    let mut full = Vec::new();
    for n in 0..35 {
        let start = Instant::now();
        let frame = service
            .prepare_frame(f.request(f.access(), vec![], Some(graph_request("tech"))))
            .unwrap();
        assert!(frame
            .frame()
            .graph
            .as_ref()
            .is_some_and(|g| !g.nodes.is_empty()));
        let build = start.elapsed();
        let check = Instant::now();
        assert_eq!(
            service.revalidate_frame(&frame).unwrap(),
            saaa_personal_state_core::world::runtime_frame::FrameValidity::Current
        );
        let checked = check.elapsed();
        let rendered = super::render::render_world_frame_explicit(frame.frame()).unwrap();
        assert!(rendered.len() <= 8_704);
        if n >= 5 {
            prepare.push(build);
            validate.push(checked);
            full.push(start.elapsed());
        }
    }
    prepare.sort();
    validate.sort();
    full.sort();
    println!(
        "G1_PERF prepare_p95={:?} revalidate_p95={:?} full_p95={:?} prepare_max={:?}",
        prepare[28], validate[28], full[28], prepare[29]
    );
    assert!(prepare[28] <= Duration::from_millis(150));
    assert!(validate[28] <= Duration::from_millis(150));
    assert!(prepare[29] <= Duration::from_millis(500));
    assert!(full[28] <= Duration::from_millis(350));
}
