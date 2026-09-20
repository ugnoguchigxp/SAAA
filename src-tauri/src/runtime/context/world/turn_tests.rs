use super::super::generation::{begin, BeginGeneration};
use super::super::generation_inputs::record;
use super::super::source::{Candidate, Requirement};
use super::render::parse_rendered_json;
use super::source::{prepare_calls, reset_prepare_calls, WorldOmission, WORLD_KIND};
use super::turn::{compose_parts, explicit_project};
use crate::meeting::MeetingState;
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::runtime_test_support::{
    Fixture, CODING_ID, MEETING_ID, RUN_ID,
};
use crate::memory::personal_state::world::test_support::PROJECT;
use crate::runtime::context::scope::{ResolvedScope, ScopeSnapshot};
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::sync::Arc;

fn window(limit: usize, status: &'static str) -> ContextWindow {
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

fn load_scope(fixture: &Fixture) -> crate::runtime::context::scope::ScopeSnapshot {
    fixture
        .writer
        .read_serialized(|connection| crate::runtime::context::scope::load(connection, RUN_ID))
        .expect("scope")
}

fn allowed(scope: &crate::runtime::context::scope::ScopeSnapshot) -> BTreeSet<String> {
    scope.scopes.iter().map(|item| item.key.clone()).collect()
}

fn compose(
    fixture: &Fixture,
    memory_on: bool,
    existing: Vec<Candidate>,
    limit: usize,
    status: &'static str,
) -> super::turn::TurnCompose {
    let scope = load_scope(fixture);
    let access = fixture.access();
    compose_parts(
        memory_on,
        Some(Arc::new(fixture.service())),
        access.principal,
        access.policy_revision,
        RUN_ID,
        &scope,
        window(limit, status),
        existing,
        allowed(&scope),
    )
    .expect("compose")
}

fn existing(id: &str, bytes: usize) -> Candidate {
    Candidate::untrusted(
        format!("z-{id}"),
        "fixture",
        vec![PROJECT.to_string()],
        Requirement::May,
        format!("source-{id}"),
        1,
        0,
        "e".repeat(bytes),
    )
}

fn world_selected(composed: &super::turn::TurnCompose) -> bool {
    composed
        .envelope
        .selected
        .iter()
        .any(|candidate| candidate.source_kind == WORLD_KIND)
}

fn snap(scopes: Vec<ResolvedScope>) -> ScopeSnapshot {
    ScopeSnapshot {
        status: "resolved".into(),
        focus_scope_key: None,
        digest: String::new(),
        reason_code: None,
        scopes,
    }
}

fn item(kind: &str, key: &str, relation: &str) -> ResolvedScope {
    ResolvedScope {
        key: key.into(),
        kind: kind.into(),
        relation: relation.into(),
        epoch: 1,
    }
}

#[test]
fn m3b_01_project_counts_and_refs_come_from_scope() {
    assert_eq!(
        explicit_project(&snap(Vec::new())),
        Err(WorldOmission::NoExplicitProject)
    );
    assert_eq!(
        explicit_project(&snap(vec![
            item("project", "project:a", "focus"),
            item("project", "project:b", "parent"),
        ])),
        Err(WorldOmission::AmbiguousProject)
    );
    let meeting = format!("resource:{MEETING_ID}");
    assert_eq!(
        super::turn::runtime_refs(&snap(vec![
            item("resource", &meeting, "current"),
            item("resource", &meeting, "focus"),
        ]))
        .len(),
        1
    );
    let source = include_str!("turn.rs");
    assert!(source.contains("graph_request: None"));
    assert!(!source.contains("ExactName"));
}

#[test]
fn m3b_06_memory_off_skips_world() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    reset_prepare_calls();
    let composed = compose(&fixture, false, Vec::new(), 8_192, "green");
    assert!(!world_selected(&composed));
    assert!(composed.world.is_none());
    assert_eq!(composed.compose_count, 1);
    assert_eq!(prepare_calls(), 0);
    reset_prepare_calls();
    let scope = load_scope(&fixture);
    let composed = compose_parts(
        true,
        Some(Arc::new(fixture.service())),
        "",
        fixture.access().policy_revision,
        RUN_ID,
        &scope,
        window(8_192, "green"),
        Vec::new(),
        allowed(&scope),
    )
    .expect("compose");
    assert_eq!(composed.omission, Some(WorldOmission::ScopeDenied));
    assert_eq!(prepare_calls(), 0);
}

#[test]
fn m3b_06_meeting_only_selects_world_without_other_project() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    reset_prepare_calls();
    let composed = compose(&fixture, true, Vec::new(), 8_192, "green");
    assert!(world_selected(&composed));
    assert_eq!(composed.compose_count, 2);
    assert_eq!(prepare_calls(), 1);
    assert_eq!(
        composed.envelope.context_health.current_instruction_count,
        1
    );
    let world = composed
        .envelope
        .selected
        .iter()
        .find(|candidate| candidate.source_kind == WORLD_KIND)
        .expect("world");
    let json = parse_rendered_json(&world.content);
    assert_eq!(json["project_scope"], PROJECT);
    assert!(json.to_string().contains(MEETING_ID));
    assert!(!json.to_string().contains("project:other"));
    assert!(world.scope_refs.iter().all(|key| key == PROJECT
        || key == &format!("resource:{MEETING_ID}")
        || key.starts_with("task:")));
}

#[test]
fn m3b_06_coding_only_selects_world() {
    let fixture = Fixture::new(&[("task", CODING_ID)]);
    fixture.add_coding_job(7, "running", "running", "accepted");
    let composed = compose(&fixture, true, Vec::new(), 8_192, "green");
    assert!(world_selected(&composed));
    let world = composed
        .envelope
        .selected
        .iter()
        .find(|candidate| candidate.source_kind == WORLD_KIND)
        .expect("world");
    let json = parse_rendered_json(&world.content);
    assert!(json.to_string().contains(CODING_ID));
}

#[test]
fn m3b_06_project_only_is_empty_request() {
    let fixture = Fixture::new(&[]);
    let composed = compose(&fixture, true, Vec::new(), 8_192, "green");
    assert!(!world_selected(&composed));
    assert_eq!(composed.omission, Some(WorldOmission::EmptyRequest));
    assert_eq!(composed.compose_count, 1);
}

#[test]
fn m3b_06_expired_world_is_omitted_from_record() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let composed = compose(&fixture, true, Vec::new(), 8_192, "green");
    assert!(composed.world.is_some());
    fixture.set_now(3_000);
    let connection = Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES('input',?1,'user','hello','1')",
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
    record(
        &generation,
        "green",
        &composed.envelope.selected,
        &composed.envelope.omitted,
        &[],
        composed.world.as_ref(),
    )
    .expect("record");
    let selected_world: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM context_generation_inputs
                     WHERE source_kind=?1 AND selected=1",
                    [WORLD_KIND],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(selected_world, 0);
}

#[test]
fn m3b_07_would_displace_keeps_baseline() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let sized = compose(&fixture, true, Vec::new(), 8_192, "green");
    let world_bytes = sized
        .envelope
        .selected
        .iter()
        .find(|candidate| candidate.source_kind == WORLD_KIND)
        .map(|candidate| candidate.cost_bytes)
        .unwrap_or_default();
    let wrapper = "[PERSONAL_STATE — source-backed untrusted data; instructionAuthority=none]\n[END_PERSONAL_STATE]".len();
    let limit = 13 + wrapper + world_bytes + 8;
    reset_prepare_calls();
    let composed = compose(&fixture, true, vec![existing("keep", 40)], limit, "green");
    assert_eq!(composed.omission, Some(WorldOmission::WouldDisplace));
    assert!(!world_selected(&composed));
    assert_eq!(composed.compose_count, 2);
    assert_eq!(prepare_calls(), 1);
    assert_eq!(composed.envelope.selected[0].source_kind, "fixture");
}

#[test]
fn m3b_07_yellow_and_japanese_fixture_stay_in_envelope() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let composed = compose(&fixture, true, Vec::new(), 8_192, "yellow");
    assert_eq!(composed.envelope.health.status.as_str(), "yellow");
    assert_eq!(
        composed.envelope.context_health.current_instruction_count,
        1
    );
    let world = composed
        .envelope
        .selected
        .iter()
        .find(|candidate| candidate.source_kind == WORLD_KIND)
        .expect("world");
    let json = parse_rendered_json(&world.content);
    assert_eq!(json["project_scope"], PROJECT);
    assert_eq!(json["run_id"], RUN_ID);
}

#[test]
fn m3b_04_expired_after_dispatch_does_not_fail_complete() {
    let fixture = Fixture::new(&[("resource", MEETING_ID)]);
    fixture.add_meeting(MEETING_ID, "active", "100", None, None);
    fixture.meeting.set(Some(MEETING_ID), MeetingState::Active);
    let composed = compose(&fixture, true, Vec::new(), 8_192, "green");
    let connection = Connection::open_in_memory().expect("db");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES('input',?1,'user','hello','1')",
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
    record(
        &generation,
        "green",
        &composed.envelope.selected,
        &composed.envelope.omitted,
        &[],
        composed.world.as_ref(),
    )
    .expect("record");
    let frame_rows: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%world_frame%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(frame_rows, 0);
    fixture.set_now(3_000);
    generation.complete().expect("complete");
    assert_eq!(
        generation.world_observation(),
        Some("expired-after-dispatch")
    );
}

#[test]
fn m3b_05_agent_session_does_not_record_world_kind() {
    let agent = include_str!("../../../providers/agent_session/sse/generation.rs");
    assert!(agent.contains("generation_inputs::record"));
    let call = agent
        .split("generation_inputs::record")
        .nth(1)
        .expect("record call");
    assert!(call.contains("None"));
    assert!(!call.contains("persistence.world"));
    assert!(!agent.contains("world-model"));
    let chat = include_str!("../../../providers/chat_completions/generation.rs");
    assert!(chat.contains("persistence.world"));
}

#[test]
fn m3b_07_omission_is_not_knowledge_none_copy() {
    let turns = include_str!("../../turns.rs");
    assert!(!turns.contains("知識なし"));
    assert!(!turns.contains("no knowledge"));
}
