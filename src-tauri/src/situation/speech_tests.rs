use super::{holds_speech, inspect_tts_hold, speech_holds_tts};
use crate::ipc_contract::RuntimeEvent;
use crate::persistence::schema::initialize_database;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::runtime::voice_response;
use crate::situation::contracts::{
    CalendarSignal, CalendarState, ForegroundCategory, ForegroundSignal, InputActivitySignal,
    InputActivityState, SituationRuntimeSettings, TimeBucket,
};
use crate::situation::SituationSample;
use crate::test_support::app_state;
use crate::voice_behavior::{self, begin_run};
use crate::{AppState, RunCancellation, StartTurnInput};
use rusqlite::Connection;
use std::sync::Arc;

#[derive(Clone)]
struct VoiceOn;

impl RuntimeEventSender for VoiceOn {
    fn send(&self, _event: RuntimeEvent) -> tauri::Result<()> {
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }

    fn voice_response_enabled(&self) -> bool {
        true
    }
}

fn state() -> AppState {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    app_state(connection)
}

fn voice_input(run_id: &str) -> StartTurnInput {
    StartTurnInput {
        run_id: run_id.to_string(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
        content: "今の状況は".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "voice".to_string(),
        presentation_mode: "visual-and-spoken".to_string(),
    }
}

fn ready_run(state: &AppState, run_id: &str) -> StartTurnInput {
    let input = voice_input(run_id);
    assert!(begin_run(state, &input.run_id, &input.conversation_id).expect("run begins"));
    input
}

fn tts_held_count(state: &AppState, run_id: &str) -> usize {
    state
        .sqlite_readers
        .read(|connection| {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM audit_events
                     WHERE event_name='tts-held' AND runtime_run_id=?1",
                    [run_id],
                    |row| row.get(0),
                )
                .expect("count");
            Ok(count as usize)
        })
        .expect("read")
}

fn sample(
    category: ForegroundCategory,
    calendar: CalendarState,
    observed_ms: u128,
) -> SituationSample {
    SituationSample {
        foreground: ForegroundSignal {
            category,
            health: crate::situation::contracts::SignalHealth::Ready,
        },
        calendar: CalendarSignal {
            state: calendar,
            time_bucket: TimeBucket::Now,
            health: crate::situation::contracts::SignalHealth::Ready,
        },
        input_activity: InputActivitySignal {
            state: InputActivityState::Active,
            health: crate::situation::contracts::SignalHealth::Ready,
        },
        observed_at: observed_ms.to_string(),
        observed_ms,
    }
}

#[test]
fn st_01_reads_memory_without_reclassify() {
    let state = state();
    assert!(!speech_holds_tts(&state));
    state
        .situation
        .set_scene_attention_for_test("MEETING", "OBSERVE");
    let hold = inspect_tts_hold(&state).expect("hold");
    assert_eq!(hold.scene, "MEETING");
    assert_eq!(hold.proposed_attention, "OBSERVE");
    let source = include_str!("speech.rs");
    assert!(!source.contains("snapshot("));
    assert!(!source.contains("classify"));
    assert!(!source.contains("list_history"));
}

#[test]
fn st_02_holds_only_meeting_ignore_or_observe() {
    assert!(holds_speech("MEETING", "IGNORE"));
    assert!(holds_speech("MEETING", "OBSERVE"));
    assert!(!holds_speech("MEETING", "RESPOND"));
    assert!(!holds_speech("MEETING", "SUGGEST"));
    assert!(!holds_speech("FOCUS", "OBSERVE"));
    assert!(!holds_speech("UNKNOWN", "IGNORE"));
    assert!(!holds_speech("CODING", "SUGGEST"));
    let state = state();
    state
        .situation
        .set_scene_attention_for_test("FOCUS", "OBSERVE");
    assert!(!speech_holds_tts(&state));
    state
        .situation
        .set_scene_attention_for_test("UNKNOWN", "IGNORE");
    assert!(!speech_holds_tts(&state));
    state
        .situation
        .set_scene_attention_for_test("MEETING", "RESPOND");
    assert!(!speech_holds_tts(&state));
}

#[test]
fn st_03_hold_blocks_voice_start_and_streaming_policy() {
    let state = state();
    state
        .situation
        .set_scene_attention_for_test("MEETING", "OBSERVE");
    let start_input = ready_run(&state, "st-03-hold");
    let handle = voice_response::start(
        &state,
        &start_input,
        &VoiceOn,
        Arc::new(RunCancellation::default()),
    );
    assert!(handle.is_none());
    voice_behavior::end_run(&state, &start_input.run_id);
    let stream_input = voice_input("st-03-stream");
    let (streaming, enabled) =
        voice_behavior::begin_turn_speech_policy(&state, &stream_input).expect("policy");
    assert!(!streaming);
    assert!(!enabled);
    assert_eq!(
        voice_behavior::effective_presentation(
            &state,
            Some(&stream_input.run_id),
            &stream_input.conversation_id,
        )
        .expect("presentation")
        .reason_code,
        "situation_hold"
    );
    assert_eq!(tts_held_count(&state, &stream_input.run_id), 1);
    voice_behavior::end_run(&state, &stream_input.run_id);
}

#[test]
fn st_03_non_hold_allows_speak_policy() {
    let state = state();
    let input = voice_input("st-03-open");
    let (streaming, enabled) =
        voice_behavior::begin_turn_speech_policy(&state, &input).expect("policy");
    assert!(streaming);
    assert!(enabled);
    let handle = voice_response::start(
        &state,
        &input,
        &VoiceOn,
        Arc::new(RunCancellation::default()),
    );
    assert!(handle.is_some());
    handle.expect("spawned").abort();
    voice_behavior::end_run(&state, &input.run_id);
}

#[test]
fn st_04_held_audit_has_no_body() {
    let state = state();
    state
        .situation
        .set_scene_attention_for_test("MEETING", "IGNORE");
    let input = ready_run(&state, "st-04-audit");
    let _ =
        voice_behavior::effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation");
    assert_eq!(tts_held_count(&state, &input.run_id), 1);
    let _ =
        voice_behavior::effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("second presentation");
    assert_eq!(tts_held_count(&state, &input.run_id), 1);
    let json = state
        .sqlite_readers
        .read(|connection| {
            let attributes: String = connection
                .query_row(
                    "SELECT attributes_json FROM audit_events WHERE event_name='tts-held'",
                    [],
                    |row| row.get(0),
                )
                .expect("attributes");
            Ok(attributes)
        })
        .expect("read");
    assert!(json.contains("MEETING"));
    assert!(json.contains("IGNORE"));
    assert!(!json.contains("transcript"));
    assert!(!json.contains(&input.content));
    voice_behavior::end_run(&state, &input.run_id);
}

#[test]
fn st_05_start_none_then_some_after_hysteresis() {
    let state = state();
    state
        .situation
        .configure(SituationRuntimeSettings {
            enabled: true,
            ..SituationRuntimeSettings::default()
        })
        .expect("enable");
    for observed_ms in [11_000_u128, 13_000, 15_000] {
        state
            .situation
            .tick_sampled(
                &state.sqlite_writer,
                sample(
                    ForegroundCategory::Communication,
                    CalendarState::MeetingLikely,
                    observed_ms,
                ),
            )
            .expect("meeting tick");
    }
    assert!(speech_holds_tts(&state));
    let held = ready_run(&state, "st-05-held");
    let handle = voice_response::start(
        &state,
        &held,
        &VoiceOn,
        Arc::new(RunCancellation::default()),
    );
    assert!(handle.is_none());
    voice_behavior::end_run(&state, &held.run_id);

    for observed_ms in [26_000_u128, 28_000, 30_000] {
        state
            .situation
            .tick_sampled(
                &state.sqlite_writer,
                sample(ForegroundCategory::Coding, CalendarState::Free, observed_ms),
            )
            .expect("coding tick");
    }
    assert!(!speech_holds_tts(&state));
    let open = ready_run(&state, "st-05-open");
    let handle = voice_response::start(
        &state,
        &open,
        &VoiceOn,
        Arc::new(RunCancellation::default()),
    );
    assert!(handle.is_some());
    handle.expect("spawned").abort();
    voice_behavior::end_run(&state, &open.run_id);
}

#[test]
fn st_06_shadow_invariants_and_no_side_channels() {
    let state = state();
    state
        .situation
        .set_scene_attention_for_test("MEETING", "OBSERVE");
    let input = ready_run(&state, "st-06-shadow");
    let _ =
        voice_behavior::effective_presentation(&state, Some(&input.run_id), &input.conversation_id);
    let inner = state.situation.inner.lock().expect("lock");
    assert_eq!(inner.decision.mode, "shadow");
    assert_eq!(inner.decision.actual_execution, "NONE");
    assert_eq!(inner.decision.actual_presentation, "SILENT");
    drop(inner);
    let source = include_str!("speech.rs");
    for forbidden in [
        "start_meeting",
        "blocks_tts()",
        "snapshot_locked",
        "role_routing",
        "notification",
    ] {
        assert!(
            !source.contains(forbidden),
            "TTS gate must not contain {forbidden}"
        );
    }
    voice_behavior::end_run(&state, &input.run_id);
}
