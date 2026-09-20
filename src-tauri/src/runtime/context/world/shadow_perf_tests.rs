use super::super::broker::{self, BrokerInput};
use super::render::render_world_frame;
use super::shadow::{run_shadow, ShadowInput};
use super::source::WorldSourceRequest;
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::runtime_test_support::{Fixture, RUN_ID};
use crate::memory::personal_state::world::test_support::PROJECT;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const WARM_UP: usize = 5;
const SAMPLES: usize = 30;

fn p95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[28]
}

fn window() -> ContextWindow {
    ContextWindow {
        messages: vec![
            ProjectedContextMessage {
                role: "system".into(),
                content: "policy".into(),
            },
            ProjectedContextMessage {
                role: "user".into(),
                content: "current".into(),
            },
        ],
        continuity_groups: Vec::new(),
        health: ContextHealthReport {
            status: "green",
            hard_limit_bytes: 64_000,
            provider_capacity_bytes: 96_000,
            output_reserve_bytes: 20_000,
            safety_margin_bytes: 12_000,
            projected_bytes: 13,
            loaded_source_messages: 1,
            source_history_truncated: false,
            recent_source_messages: 0,
            continuity_group_count: 0,
            continuity_source_messages: 0,
            memory_item_count: 0,
            omitted_memory_items: 0,
            omitted_loaded_source_messages: 0,
            current_instruction_count: 1,
            repair_count: 0,
        },
    }
}

#[test]
#[ignore]
fn m3_19_shadow_path_stays_within_dev_gates() {
    let coding_ids: Vec<String> = (1..=8).map(|index| format!("m{index}")).collect();
    let targets: Vec<(&str, &str)> = coding_ids.iter().map(|id| ("task", id.as_str())).collect();
    let fixture = Fixture::with_entities(&targets, 100);
    let ledger = fixture.ledger_count();
    if ledger < 2_000 {
        fixture.fill_coverage(2_000 - ledger as usize);
    }
    let runtime_refs: Vec<_> = (1..=8)
        .map(|index| fixture.coding_ref(&format!("m{index}")))
        .collect();
    let mut allowed = BTreeSet::from([PROJECT.to_string()]);
    for index in 1..=8 {
        allowed.insert(format!("resource:m{index}"));
    }
    let service = fixture.service();
    let input = ShadowInput {
        run_id: RUN_ID.into(),
        base: window(),
        existing_candidates: Vec::new(),
        source_warning: None,
        allowed_scope_keys: allowed.clone(),
    };
    let mut extra_samples = Vec::new();
    let mut full_samples = Vec::new();
    let mut selected = 0usize;
    let mut prepared = 0usize;
    for round in 0..(WARM_UP + SAMPLES) {
        let access = fixture.access();
        let request = fixture.request(
            access,
            runtime_refs.clone(),
            Some(fixture.graph_request("ent0")),
        );
        let full_start = Instant::now();
        let scope = fixture
            .writer
            .read_serialized(|connection| crate::runtime::context::scope::load(connection, RUN_ID))
            .expect("scope");
        let summary = run_shadow(
            &service,
            WorldSourceRequest {
                frame_request: request,
            },
            &input,
            &|| 1_000,
            &scope,
        );
        let full = full_start.elapsed();
        prepared += 1;
        if summary.world_selected {
            selected += 1;
        }
        let access = fixture.access();
        let request = fixture.request(
            access,
            runtime_refs.clone(),
            Some(fixture.graph_request("ent0")),
        );
        let prepared_frame = service.prepare_frame(request).expect("frame");
        let extra_start = Instant::now();
        let rendered = render_world_frame(prepared_frame.frame());
        let _ = broker::compose(BrokerInput {
            base: window(),
            candidates: Vec::new(),
            source_warning: None,
            allowed_scope_keys: allowed.clone(),
        });
        if let Ok(content) = rendered {
            let candidate = super::source::frame_candidate(
                prepared_frame.frame(),
                &content,
                super::source::WORLD_SHADOW_KIND,
            );
            let _ = broker::compose(BrokerInput {
                base: window(),
                candidates: vec![candidate],
                source_warning: None,
                allowed_scope_keys: allowed.clone(),
            });
        }
        let extra = extra_start.elapsed();
        if round >= WARM_UP {
            extra_samples.push(extra);
            full_samples.push(full);
        }
    }
    let extra_p95 = p95(&mut extra_samples);
    let full_p95 = p95(&mut full_samples);
    let full_max = *full_samples.iter().max().expect("samples");
    eprintln!(
        "M3_PERF extra_p95_ms={:.3} full_p95_ms={:.3} max_ms={:.3} prepared={prepared} selected={selected} ledger={}",
        extra_p95.as_secs_f64() * 1_000.0,
        full_p95.as_secs_f64() * 1_000.0,
        full_max.as_secs_f64() * 1_000.0,
        fixture.ledger_count()
    );
    assert!(
        extra_p95 <= Duration::from_millis(20),
        "extra p95 {extra_p95:?} > 20ms"
    );
    assert!(
        full_p95 <= Duration::from_millis(350),
        "full p95 {full_p95:?} > 350ms"
    );
    assert!(full_max < Duration::from_millis(1_000));
    assert!(prepared > 0);
}
