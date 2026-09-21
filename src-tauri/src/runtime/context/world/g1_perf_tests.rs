//! Real-clock performance; unlike lifetime tests this never measures a fake clock.
use super::g1_tests::{g1_fixture, graph_request};
use std::time::{Duration, Instant};

#[test]
#[ignore = "performance gate: run explicitly on a quiet development host"]
fn world_g1_performance() {
    let f = g1_fixture();
    let service = crate::memory::personal_state::world::runtime_frame::WorldFrameService::new(
        f.readers(), std::sync::Arc::new(crate::memory::personal_state::now));
    let mut prepare = Vec::new();
    let mut validate = Vec::new();
    let mut full = Vec::new();
    for n in 0..35 {
        let start = Instant::now();
        let frame = service.prepare_frame(f.request(f.access(), vec![], Some(graph_request("tech")))).unwrap();
        let build = start.elapsed();
        let check = Instant::now();
        assert_eq!(service.revalidate_frame(&frame).unwrap(), saaa_personal_state_core::world::runtime_frame::FrameValidity::Current);
        let checked = check.elapsed();
        let rendered = super::render::render_world_frame_explicit(frame.frame()).unwrap();
        assert!(rendered.len() <= 8_704);
        if n >= 5 { prepare.push(build); validate.push(checked); full.push(start.elapsed()); }
    }
    prepare.sort(); validate.sort(); full.sort();
    println!("G1_PERF prepare_p95={:?} revalidate_p95={:?} full_p95={:?} prepare_max={:?}",prepare[28],validate[28],full[28],prepare[29]);
    assert!(prepare[28] <= Duration::from_millis(150));
    assert!(validate[28] <= Duration::from_millis(150));
    assert!(prepare[29] <= Duration::from_millis(500));
    assert!(full[28] <= Duration::from_millis(350));
}
