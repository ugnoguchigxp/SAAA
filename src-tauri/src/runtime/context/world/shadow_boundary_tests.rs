use super::super::broker::{self, BrokerInput};
use super::super::source::{Candidate, Requirement};
use super::shadow::{run_shadow, ShadowInput};
use super::source::WorldSourceRequest;
use crate::meeting::MeetingState;
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::runtime_test_support::{Fixture, MEETING_ID, RUN_ID};
use crate::memory::personal_state::world::test_support::PROJECT;
use std::collections::BTreeSet;

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
            hard_limit_bytes: 8_192,
            provider_capacity_bytes: 40_192,
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
fn m3_18_shadow_does_not_write_or_dispatch() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let generations = fixture.table_count("context_generations");
    let changes = fixture.total_changes();
    let service = fixture.service();
    let access = fixture.access();
    let request = fixture.request(access, vec![fixture.meeting_ref(MEETING_ID)], None);
    let input = ShadowInput {
        run_id: RUN_ID.into(),
        base: window(),
        existing_candidates: Vec::new(),
        source_warning: None,
        allowed_scope_keys: BTreeSet::from([PROJECT.to_string(), format!("resource:{MEETING_ID}")]),
    };
    let scope = fixture
        .writer
        .read_serialized(|connection| crate::runtime::context::scope::load(connection, RUN_ID))
        .expect("scope");
    let _ = run_shadow(
        &service,
        WorldSourceRequest {
            frame_request: request,
        },
        &input,
        &|| 1_000,
        &scope,
    );
    assert_eq!(fixture.table_count("context_generations"), generations);
    assert_eq!(fixture.total_changes(), changes);
    let baseline = broker::compose(BrokerInput {
        base: window(),
        candidates: vec![Candidate::untrusted(
            "fixture-a".into(),
            "fixture",
            vec![PROJECT.into()],
            Requirement::May,
            "source-a".into(),
            1,
            0,
            "hello".into(),
        )],
        source_warning: None,
        allowed_scope_keys: BTreeSet::from([PROJECT.to_string()]),
    })
    .unwrap();
    assert_eq!(baseline.selected[0].source_kind, "fixture");
    let turns = include_str!("../../turns.rs");
    let controller = include_str!("../../conversation_controller/mod.rs");
    let chat = include_str!("../../../providers/chat_completions/mod.rs");
    let agent = include_str!("../../../providers/agent_session.rs");
    assert!(turns.contains("compose_for_app"));
    for source in [turns, controller, chat, agent] {
        assert!(!source.contains("world-model-shadow"));
        assert!(!source.contains("run_shadow"));
        assert!(!source.contains("prepare_candidate"));
    }
}
