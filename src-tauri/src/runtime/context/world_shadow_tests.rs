use super::broker::{self, BrokerInput};
use super::generation::{begin, BeginGeneration};
use super::generation_inputs::record;
use super::source::{Candidate, Requirement};
use super::world_shadow::{run_shadow, ShadowInput, ShadowStatus};
use super::world_source::{
    prepare_calls, reset_prepare_calls, WorldOmission, WorldSourceRequest, WORLD_SHADOW_KIND,
};
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::runtime_test_support::{
    Fixture, CODING_ID, MEETING_ID, RUN_ID,
};
use crate::memory::personal_state::world::test_support::PROJECT;
use crate::meeting::MeetingState;
use rusqlite::Connection;
use std::collections::BTreeSet;

fn base(limit: usize, status: &'static str) -> ContextWindow {
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
            status,
            hard_limit_bytes: limit,
            provider_capacity_bytes: limit + 32_000,
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

fn existing(id: &str, bytes: usize, utility: u16) -> Candidate {
    Candidate::untrusted(
        format!("z-{id}"),
        "fixture",
        vec![PROJECT.to_string()],
        Requirement::May,
        format!("source-{id}"),
        1,
        utility,
        "e".repeat(bytes),
    )
}

fn shadow_input(fixture: &Fixture, limit: usize, candidates: Vec<Candidate>) -> ShadowInput {
    let mut allowed = BTreeSet::from([PROJECT.to_string()]);
    allowed.insert(format!("resource:{MEETING_ID}"));
    allowed.insert(format!("task:{CODING_ID}"));
    ShadowInput {
        run_id: RUN_ID.into(),
        base: base(limit, "green"),
        existing_candidates: candidates,
        source_warning: None,
        allowed_scope_keys: allowed,
    }
}

fn shadow(
    fixture: &Fixture,
    runtime: bool,
    graph: bool,
    input: &ShadowInput,
    clock: i64,
) -> super::world_shadow::ShadowSummary {
    let service = fixture.service();
    let access = fixture.access();
    let refs = if runtime {
        vec![fixture.meeting_ref(MEETING_ID)]
    } else {
        Vec::new()
    };
    let graph = if graph {
        Some(fixture.graph_request("ent0"))
    } else {
        None
    };
    let request = fixture.request(access, refs, graph);
    fixture
        .writer
        .read_serialized(|connection| {
            Ok(run_shadow(
                &service,
                connection,
                WorldSourceRequest {
                    frame_request: request,
                },
                input,
                &|| clock,
            ))
        })
        .expect("shadow")
}

#[test]
fn m3_08_summary_has_no_payload_and_input_stays_put() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&fixture, 8_192, Vec::new());
    let before = input.clone();
    let summary = shadow(&fixture, true, false, &input, 1_000);
    assert_eq!(input, before);
    assert!(matches!(summary.status, ShadowStatus::Compared));
    let encoded = format!("{summary:?}");
    assert!(!encoded.contains("会議"));
    assert!(!encoded.contains(PROJECT));
}

#[test]
fn m3_09_baseline_matches_broker_and_skips_prepare_on_error() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&fixture, 8_192, vec![existing("a", 8, 0)]);
    let envelope = broker::compose(BrokerInput {
        base: input.base.clone(),
        candidates: input.existing_candidates.clone(),
        source_warning: input.source_warning.clone(),
        allowed_scope_keys: input.allowed_scope_keys.clone(),
    })
    .unwrap();
    let summary = shadow(&fixture, true, false, &input, 1_000);
    assert_eq!(summary.baseline_bytes, Some(envelope.health.projected_bytes));
    assert_eq!(summary.existing_selected_count, envelope.selected.len());

    reset_prepare_calls();
    let mut broken = input.clone();
    broken.base.messages.pop();
    let summary = shadow(&fixture, true, false, &broken, 1_000);
    assert_eq!(summary.status, ShadowStatus::BaselineError);
    assert_eq!(prepare_calls(), 0);
}

#[test]
fn m3_10_partial_scope_is_denied_without_second_compose_success() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let mut input = shadow_input(&fixture, 8_192, Vec::new());
    input.allowed_scope_keys = BTreeSet::from([PROJECT.to_string()]);
    let summary = shadow(&fixture, true, false, &input, 1_000);
    assert_eq!(summary.omission, Some(WorldOmission::ScopeDenied));
}

#[test]
fn m3_11_same_utility_does_not_displace_existing() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&fixture, 80, vec![existing("z", 40, 0)]);
    let summary = shadow(&fixture, true, false, &input, 1_000);
    assert_eq!(summary.omission, Some(WorldOmission::WouldDisplace));
    assert!(!summary.world_selected);
    assert_eq!(summary.proposed_bytes, summary.baseline_bytes);

    let fits = shadow_input(&fixture, 8_192, vec![existing("z", 8, 0)]);
    let summary = shadow(&fixture, true, false, &fits, 1_000);
    assert_eq!(summary.status, ShadowStatus::Compared);
    assert!(summary.world_selected);
}

#[test]
fn m3_12_expired_clock_and_other_run_do_not_retry() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&fixture, 8_192, Vec::new());
    reset_prepare_calls();
    let summary = shadow(&fixture, true, false, &input, 2_000);
    assert_eq!(summary.omission, Some(WorldOmission::Expired));
    assert_eq!(prepare_calls(), 1);

    reset_prepare_calls();
    let mut other = input.clone();
    other.run_id = "run-other".into();
    let summary = shadow(&fixture, true, false, &other, 1_000);
    assert_eq!(summary.omission, Some(WorldOmission::ScopeDenied));
    assert_eq!(prepare_calls(), 0);
}

#[test]
fn m3_13_shadow_kind_is_rejected_before_record() {
    let connection = Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES('input',?1,'user','secret','1')",
            [crate::PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at)
             VALUES('run',?1,'conversation.respond','running','input','1')",
            [crate::PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    let state = crate::test_support::app_state(connection);
    let generation = begin(
        &state,
        BeginGeneration {
            run_id: "run",
            provider_session_id: None,
            provider_id: Some("provider"),
            purpose: "reasoning",
            request_payload: b"request",
            envelope_payload: b"request",
            current_instruction_count: 1,
        },
    )
    .unwrap();
    let shadow = Candidate::untrusted(
        "world-frame:run:abc".into(),
        WORLD_SHADOW_KIND,
        vec![PROJECT.into()],
        Requirement::May,
        "world-frame:run:abc".into(),
        1,
        0,
        "payload".into(),
    );
    let before = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT status, COUNT(*) FROM context_generations g
                     LEFT JOIN context_generation_inputs i ON i.generation_id=g.id
                     WHERE g.run_id='run'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    let error = record(&generation, "green", &[shadow.clone()], &[], &[]).unwrap_err();
    assert_eq!(error, "world-shadow-not-dispatchable");
    let omitted_error = record(&generation, "green", &[], &[shadow], &[]).unwrap_err();
    assert_eq!(omitted_error, "world-shadow-not-dispatchable");
    let after = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT status, COUNT(*) FROM context_generations g
                     LEFT JOIN context_generation_inputs i ON i.generation_id=g.id
                     WHERE g.run_id='run'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.0, "planned");
    drop(generation);
}

#[test]
fn m3_14_real_frame_paths_select_without_fake_candidates() {
    let graph = Fixture::with_entities(&[], 2);
    let input = shadow_input(&graph, 8_192, Vec::new());
    let summary = shadow(&graph, false, true, &input, 1_000);
    assert_eq!(summary.status, ShadowStatus::Compared);
    assert!(summary.world_selected);

    let runtime = Fixture::new(&[("resource", MEETING_ID)]);
    runtime.add_meeting(MEETING_ID, "active", "100", None, None);
    runtime.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&runtime, 8_192, Vec::new());
    let summary = shadow(&runtime, true, false, &input, 1_000);
    assert!(summary.world_selected);

    let both = Fixture::with_entities(&[("resource", MEETING_ID)], 2);
    both.add_meeting(MEETING_ID, "active", "100", None, None);
    both.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&both, 8_192, Vec::new());
    let summary = shadow(&both, true, true, &input, 1_000);
    assert!(summary.world_selected);
}

#[test]
fn m3_15_source_and_link_changes_drop_stale_candidates() {
    let fixture = Fixture::with_entities(&[("resource", MEETING_ID)], 2);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let input = shadow_input(&fixture, 8_192, Vec::new());
    assert!(shadow(&fixture, true, true, &input, 1_000).world_selected);
    fixture
        .writer
        .write(|connection| {
            connection
                .execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let summary = shadow(&fixture, true, true, &input, 1_000);
    assert!(!summary.world_selected);
    assert!(summary.omission.is_some());
}

#[test]
fn m3_17_yellow_health_survives_and_instruction_stays_one() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let mut input = shadow_input(&fixture, 8_192, Vec::new());
    input.base.health.status = "yellow";
    input.source_warning = Some("fixture-warning".into());
    let envelope = broker::compose(BrokerInput {
        base: input.base.clone(),
        candidates: Vec::new(),
        source_warning: input.source_warning.clone(),
        allowed_scope_keys: input.allowed_scope_keys.clone(),
    })
    .unwrap();
    assert_eq!(envelope.health.status.as_str(), "yellow");
    let summary = shadow(&fixture, true, false, &input, 1_000);
    assert_eq!(summary.status, ShadowStatus::Compared);
    assert_eq!(envelope.context_health.current_instruction_count, 1);
    let tight = shadow_input(&fixture, 40, Vec::new());
    let omitted = shadow(&fixture, true, false, &tight, 1_000);
    assert!(
        omitted.omission == Some(WorldOmission::Budget)
            || omitted.omission == Some(WorldOmission::WouldDisplace)
            || omitted.world_selected
    );
}
