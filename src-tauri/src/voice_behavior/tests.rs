use rusqlite::Connection;
use serde_json::Value;

use super::completion::effective_presentation_from;
use super::*;
use crate::persistence::schema::initialize_database;
use crate::test_support::app_state;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
}

fn turn_input(run_id: &str) -> StartTurnInput {
    StartTurnInput {
        run_id: run_id.to_string(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
        content: "静かにして".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        input_origin: "text".to_string(),
        presentation_mode: "visual-and-spoken".to_string(),
    }
}

#[test]
fn strict_tool_arguments_reject_missing_null_and_invalid_combinations() {
    assert!(parse_tool_arguments(
        r#"{"speechOutput":{"mode":"silent","scope":"current_response"},"listeningPace":null}"#
    )
    .is_ok());
    assert!(parse_tool_arguments(r#"{"speechOutput":null,"listeningPace":null}"#).is_err());
    assert!(parse_tool_arguments(r#"{"speechOutput":null}"#).is_err());
    assert!(parse_tool_arguments(
        r#"{"speechOutput":{"mode":"speak","scope":"conversation"},"listeningPace":null}"#
    )
    .is_err());
    assert!(parse_tool_arguments(
        r#"{"speechOutput":null,"listeningPace":{"mode":"patient","scope":"current_response"}}"#
    )
    .is_err());
    assert!(parse_tool_arguments(
        r#"{"speechOutput":null,"listeningPace":{"mode":"patient","scope":"conversation"},"extra":true}"#
    )
    .is_err());
}

#[test]
fn policy_is_conversation_scoped_and_deleted_with_its_conversation() {
    let connection = connection();
    let first =
        ensure_policy(&connection, crate::PRIMARY_CONVERSATION_ID).expect("primary policy loads");
    assert_eq!(first.speech_output_override, "inherit");
    connection
        .execute(
            "INSERT INTO conversations(id,task_mode,created_at,updated_at)
             VALUES('conversation_other','conversation','1','1')",
            [],
        )
        .expect("conversation inserts");
    let other = ensure_policy(&connection, "conversation_other").expect("policy loads");
    assert_eq!(other.policy_revision, 1);
    connection
        .execute(
            "DELETE FROM conversations WHERE id='conversation_other'",
            [],
        )
        .expect("conversation deletes");
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_voice_policies WHERE conversation_id='conversation_other'",
            [],
            |row| row.get(0),
        )
        .expect("count reads");
    assert_eq!(count, 0);
}

#[test]
fn presentation_precedence_keeps_hard_and_global_blocks_above_overrides() {
    assert_eq!(
        effective_presentation_from(true, true, Some(RunSpeechOverride::Speak), "inherit")
            .reason_code,
        "meeting_blocked"
    );
    assert_eq!(
        effective_presentation_from(false, false, Some(RunSpeechOverride::Speak), "inherit")
            .reason_code,
        "global_opt_out"
    );
    assert_eq!(
        effective_presentation_from(false, true, Some(RunSpeechOverride::Speak), "muted").decision,
        "speak"
    );
    assert_eq!(
        effective_presentation_from(false, true, None, "muted").reason_code,
        "conversation_override"
    );
}

#[test]
fn tool_definition_is_strict_and_bounded() {
    let definition = tool_definition();
    assert_eq!(
        definition.pointer("/function/name").and_then(Value::as_str),
        Some(UPDATE_VOICE_BEHAVIOR_TOOL_NAME)
    );
    assert_eq!(
        definition
            .pointer("/function/strict")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        definition
            .pointer("/function/parameters/additionalProperties")
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn persistent_tool_mutation_is_idempotent_and_restored_for_the_conversation() {
    let state = app_state(connection());
    let input = turn_input("run_voice_persist");
    assert!(begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins"));
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_persist".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments: r#"{"speechOutput":{"mode":"silent","scope":"conversation"},"listeningPace":{"mode":"patient","scope":"conversation"}}"#.to_string(),
    };
    let first = execute_tool(&state, &input, &call);
    let duplicate = execute_tool(&state, &input, &call);
    assert_eq!(first, duplicate);
    let result: Value = serde_json::from_str(&first).expect("result decodes");
    assert_eq!(result.get("applied").and_then(Value::as_bool), Some(true));
    let snapshot = policy_snapshot(&state, &input.conversation_id).expect("policy loads");
    assert_eq!(snapshot.speech_output, "muted");
    assert_eq!(snapshot.listening_pace, "patient");
    assert_eq!(snapshot.policy_revision, 2);
    assert_eq!(
        snapshot.effective_silence_timeout_ms,
        PATIENT_SILENCE_TIMEOUT_MS
    );
    let event_count: i64 = state
        .sqlite_writer
        .lock()
        .expect("database locks")
        .query_row(
            "SELECT COUNT(*) FROM conversation_voice_policy_events
             WHERE runtime_run_id='run_voice_persist' AND tool_call_id='call_voice_persist'",
            [],
            |row| row.get(0),
        )
        .expect("audit count reads");
    assert_eq!(event_count, 1);
}

#[test]
fn current_response_silence_does_not_persist_to_the_next_run() {
    let state = app_state(connection());
    let input = turn_input("run_voice_once");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_once".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":{"mode":"silent","scope":"current_response"},"listeningPace":null}"#
                .to_string(),
    };
    let result = execute_tool(&state, &input, &call);
    assert!(result.contains("turn_override"));
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation resolves")
            .decision,
        "silent"
    );
    let audit_result: String = state
        .sqlite_writer
        .lock()
        .expect("database locks")
        .query_row(
            "SELECT result_code FROM conversation_voice_policy_events
             WHERE runtime_run_id=?1 AND tool_call_id=?2",
            [&input.run_id, &call.id],
            |row| row.get(0),
        )
        .expect("audit result reads");
    assert_eq!(audit_result, "applied");
    end_run(&state, &input.run_id);
    let next = turn_input("run_voice_next");
    begin_run(&state, &next.run_id, &next.conversation_id).expect("next run begins");
    assert_eq!(
        effective_presentation(&state, Some(&next.run_id), &next.conversation_id)
            .expect("presentation resolves")
            .decision,
        "speak"
    );
}

#[test]
fn stale_tool_mutation_cannot_overwrite_a_newer_ui_policy() {
    let state = app_state(connection());
    let input = turn_input("run_voice_conflict");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    let updated = update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: Some("muted".to_string()),
            listening_pace: None,
            expected_revision: 1,
        },
    )
    .expect("UI policy updates");
    assert_eq!(updated.policy_revision, 2);
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_conflict".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":null,"listeningPace":{"mode":"quick","scope":"conversation"}}"#
                .to_string(),
    };
    let result = execute_tool(&state, &input, &call);
    assert!(result.contains("policy-conflict"));
    let snapshot = policy_snapshot(&state, &input.conversation_id).expect("policy loads");
    assert_eq!(snapshot.speech_output, "muted");
    assert_eq!(snapshot.listening_pace, "inherit");
    assert_eq!(snapshot.policy_revision, 2);
}

#[test]
fn stale_persistent_silence_still_applies_to_the_current_response() {
    let state = app_state(connection());
    let input = turn_input("run_voice_silent_conflict");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: None,
            listening_pace: Some("patient".to_string()),
            expected_revision: 1,
        },
    )
    .expect("UI policy updates");
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_silent_conflict".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":{"mode":"silent","scope":"conversation"},"listeningPace":null}"#
                .to_string(),
    };

    let result = execute_tool(&state, &input, &call);
    let result: Value = serde_json::from_str(&result).expect("result decodes");
    assert_eq!(
        result.pointer("/applied").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        result
            .pointer("/outcomes/speechOutput")
            .and_then(Value::as_str),
        Some("applied-current-response-only")
    );
    assert_eq!(
        result
            .pointer("/takesEffect/speechOutput")
            .and_then(Value::as_str),
        Some("current_response")
    );
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation resolves")
            .reason_code,
        "turn_override"
    );
    let snapshot = policy_snapshot(&state, &input.conversation_id).expect("policy loads");
    assert_eq!(snapshot.speech_output, "inherit");
    assert_eq!(snapshot.listening_pace, "patient");
    assert_eq!(snapshot.policy_revision, 2);
}

#[test]
fn presentation_and_completion_fail_closed_for_invalid_run_context() {
    let state = app_state(connection());
    let input = turn_input("run_voice_context");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    state
        .sqlite_writer
        .lock()
        .expect("database locks")
        .execute(
            "INSERT INTO conversations(id,task_mode,created_at,updated_at)
             VALUES('conversation_other_voice','conversation','1','1')",
            [],
        )
        .expect("conversation inserts");

    assert!(
        effective_presentation(&state, Some(&input.run_id), "conversation_other_voice").is_err()
    );
    assert!(
        effective_presentation(&state, Some("run_voice_missing"), &input.conversation_id).is_err()
    );
    let (presentation, voice_policy) =
        completion_state(&state, "run_voice_missing", &input.conversation_id);
    assert_eq!(presentation.decision, "silent");
    assert_eq!(presentation.reason_code, "route_blocked");
    assert!(voice_policy.is_none());
}

#[test]
fn reset_rejects_non_positive_revision() {
    let state = app_state(connection());
    let error = reset_policy_from_ui(
        &state,
        ResetConversationVoicePolicyInput {
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            expected_revision: 0,
        },
    )
    .expect_err("invalid revision is rejected");
    assert_eq!(error, "Voice policy revision must be positive");
}

#[test]
fn database_failure_rolls_back_persistent_mute_but_keeps_this_response_silent() {
    let state = app_state(connection());
    let input = turn_input("run_voice_db_failure");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    state
        .sqlite_writer
        .lock()
        .expect("database locks")
        .execute_batch(
            "CREATE TRIGGER fail_voice_policy_audit
             BEFORE INSERT ON conversation_voice_policy_events
             BEGIN SELECT RAISE(ABORT, 'injected voice policy audit failure'); END;",
        )
        .expect("failure trigger installs");
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_db_failure".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":{"mode":"silent","scope":"conversation"},"listeningPace":null}"#
                .to_string(),
    };

    let result = execute_tool(&state, &input, &call);

    assert!(result.contains("voice-policy-unavailable"));
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation resolves")
            .reason_code,
        "turn_override"
    );
    let snapshot = policy_snapshot(&state, &input.conversation_id).expect("policy loads");
    assert_eq!(snapshot.speech_output, "inherit");
    assert_eq!(snapshot.policy_revision, 1);
}

#[test]
fn persistent_mute_can_resume_speech_in_the_same_turn() {
    let state = app_state(connection());
    let muted = update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            speech_output: Some("muted".to_string()),
            listening_pace: None,
            expected_revision: 1,
        },
    )
    .expect("conversation mutes");
    assert_eq!(muted.effective_speech_output, "silent");
    let input = turn_input("run_voice_resume");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_resume".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":{"mode":"inherit","scope":"conversation"},"listeningPace":null}"#
                .to_string(),
    };

    let result = execute_tool(&state, &input, &call);
    let result: Value = serde_json::from_str(&result).expect("result decodes");

    assert_eq!(
        result
            .pointer("/effective/speechOutput")
            .and_then(Value::as_str),
        Some("speak")
    );
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation resolves")
            .decision,
        "speak"
    );
}

#[test]
fn persistent_policy_is_restored_after_reopening_the_database() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    let path = directory.path().join("voice-policy.sqlite3");
    let first = Connection::open(&path).expect("database opens");
    initialize_database(&first).expect("database initializes");
    let state = app_state(first);
    update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            speech_output: Some("muted".to_string()),
            listening_pace: Some("quick".to_string()),
            expected_revision: 1,
        },
    )
    .expect("policy updates");
    drop(state);

    let reopened = Connection::open(&path).expect("database reopens");
    initialize_database(&reopened).expect("database migrations rerun");
    let restored = app_state(reopened);
    let snapshot =
        policy_snapshot(&restored, crate::PRIMARY_CONVERSATION_ID).expect("restored policy loads");

    assert_eq!(snapshot.speech_output, "muted");
    assert_eq!(snapshot.listening_pace, "quick");
    assert_eq!(snapshot.policy_revision, 2);
}

#[test]
fn manual_resume_keeps_an_already_cancelled_response_silent() {
    let state = app_state(connection());
    let input = turn_input("run_voice_manual_recovery");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");

    let muted = update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: Some("muted".to_string()),
            listening_pace: None,
            expected_revision: 1,
        },
    )
    .expect("manual mute applies");
    assert_eq!(muted.policy_revision, 2);
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("muted presentation resolves")
            .reason_code,
        "turn_override"
    );

    update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: Some("inherit".to_string()),
            listening_pace: None,
            expected_revision: 2,
        },
    )
    .expect("manual resume applies");
    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("resumed presentation resolves")
            .reason_code,
        "turn_override"
    );
    assert_eq!(
        policy_snapshot(&state, &input.conversation_id)
            .expect("resumed policy loads")
            .speech_output,
        "inherit"
    );
    let (presentation, snapshot) =
        presentation_and_snapshot(&state, Some(&input.run_id), &input.conversation_id)
            .expect("completion state resolves");
    assert_eq!(presentation.decision, "silent");
    assert_eq!(snapshot.effective_speech_output, "speak");
}

#[test]
fn manual_resume_applies_to_a_response_that_started_conversation_muted() {
    let state = app_state(connection());
    update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            speech_output: Some("muted".to_string()),
            listening_pace: None,
            expected_revision: 1,
        },
    )
    .expect("conversation mutes");
    let input = turn_input("run_voice_manual_resume");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");

    update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: Some("inherit".to_string()),
            listening_pace: None,
            expected_revision: 2,
        },
    )
    .expect("manual resume applies");

    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("resumed presentation resolves")
            .decision,
        "speak"
    );
}

#[test]
fn resetting_only_listening_pace_does_not_override_current_response_silence() {
    let state = app_state(connection());
    let input = turn_input("run_voice_pace_reset");
    begin_run(&state, &input.run_id, &input.conversation_id).expect("run begins");
    let call = crate::runtime::agent_tools::AgentToolCall {
        id: "call_voice_pace_reset".to_string(),
        name: UPDATE_VOICE_BEHAVIOR_TOOL_NAME.to_string(),
        arguments:
            r#"{"speechOutput":{"mode":"silent","scope":"current_response"},"listeningPace":null}"#
                .to_string(),
    };
    execute_tool(&state, &input, &call);
    let paced = update_policy_from_ui(
        &state,
        UpdateConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            speech_output: None,
            listening_pace: Some("quick".to_string()),
            expected_revision: 1,
        },
    )
    .expect("pace updates");

    reset_policy_from_ui(
        &state,
        ResetConversationVoicePolicyInput {
            conversation_id: input.conversation_id.clone(),
            expected_revision: paced.policy_revision,
        },
    )
    .expect("pace resets");

    assert_eq!(
        effective_presentation(&state, Some(&input.run_id), &input.conversation_id)
            .expect("presentation resolves")
            .reason_code,
        "turn_override"
    );
}
