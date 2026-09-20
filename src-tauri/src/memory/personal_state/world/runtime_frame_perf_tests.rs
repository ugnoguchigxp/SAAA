#![cfg(test)]
//! M2-27: development performance gates. Run explicitly:
//! `cargo test --lib m2_27_ -- --ignored --nocapture`
//!
//! Conditions: 100 projected entities, <=2,000 ledger records, 8 runtime refs,
//! warm-up 5, 30 samples, nearest-rank p95 (29th ascending). Fixture build time
//! is measured separately and a >5s build is not certified.

use super::runtime_test_support::*;
use saaa_personal_state_core::world::runtime_frame::RuntimeKind;
use std::time::{Duration, Instant};

const WARM_UP: usize = 5;
const SAMPLES: usize = 30;

fn p95(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples[(SAMPLES as f64 * 0.95).ceil() as usize - 1]
}

#[test]
#[ignore]
fn m2_27_frame_and_revalidation_gates() {
    let targets: Vec<(&str, &str)> = (1..=8)
        .map(|index| {
            let id: &'static str = Box::leak(format!("m{index}").into_boxed_str());
            ("task", id)
        })
        .collect();
    let build_start = Instant::now();
    let fixture = Fixture::with_entities(&targets, 100);
    let ledger = fixture.ledger_count();
    if ledger < 2_000 {
        fixture.fill_coverage(2_000 - ledger as usize);
    }
    let ledger = fixture.ledger_count();
    let build = build_start.elapsed();
    assert!(
        build < Duration::from_secs(5),
        "fixture build exceeded the 5s certification budget: {build:?}"
    );
    let runtime_refs: Vec<_> = (1..=8)
        .map(|index| fixture.coding_ref(&format!("m{index}")))
        .collect();

    let service = fixture.service();
    let measure = |label: &str, mut op: Box<dyn FnMut() + '_>| -> (Duration, Duration) {
        for _ in 0..WARM_UP {
            op();
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let start = Instant::now();
            op();
            samples.push(start.elapsed());
        }
        let p95 = p95(&mut samples);
        let max = *samples.last().unwrap();
        eprintln!(
            "M2_PERF {label} p95_ms={:.3} max_ms={:.3} ledger={ledger} refs={}",
            p95.as_secs_f64() * 1_000.0,
            max.as_secs_f64() * 1_000.0,
            runtime_refs.len()
        );
        (p95, max)
    };

    // Runtime only (no graph).
    let (runtime_p95, _) = measure(
        "runtime_only",
        Box::new(|| {
            let access = fixture.access();
            let request = fixture.request(access, runtime_refs.clone(), None);
            let _ = service.prepare_frame(request);
        }),
    );
    assert!(
        runtime_p95 <= Duration::from_millis(20),
        "runtime p95 {runtime_p95:?} > 20ms"
    );

    // Full frame with graph.
    let (frame_p95, frame_max) = measure(
        "frame",
        Box::new(|| {
            let access = fixture.access();
            let request = fixture.request(
                access,
                runtime_refs.clone(),
                Some(fixture.graph_request("ent0")),
            );
            let frame = service.prepare_frame(request).expect("frame");
            assert_eq!(frame.frame().schema_version, 1);
        }),
    );
    assert!(
        frame_p95 <= Duration::from_millis(150),
        "frame p95 {frame_p95:?}"
    );
    assert!(
        frame_max <= Duration::from_millis(500),
        "frame max {frame_max:?}"
    );

    // Revalidation of the same frame shape.
    let access = fixture.access();
    let request = fixture.request(
        access,
        runtime_refs.clone(),
        Some(fixture.graph_request("ent0")),
    );
    let prepared = service.prepare_frame(request).expect("prepared");
    let (revalidate_p95, _) = measure(
        "revalidate",
        Box::new(|| {
            let _ = service.revalidate_frame(&prepared).expect("revalidates");
        }),
    );
    assert!(
        revalidate_p95 <= Duration::from_millis(150),
        "revalidate p95 {revalidate_p95:?}"
    );

    assert_eq!(runtime_refs[0].kind, RuntimeKind::CodingJob);
}
