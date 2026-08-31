use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::{database_error, new_id, now_iso, validate_identifier, AppState, VoiceRuntimeSettings};

use super::{
    completion::effective_presentation_from, ConversationVoicePolicySnapshot,
    ResetConversationVoicePolicyInput, UpdateConversationVoicePolicyInput,
};
use super::{BALANCED_SILENCE_TIMEOUT_MS, PATIENT_SILENCE_TIMEOUT_MS, QUICK_SILENCE_TIMEOUT_MS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyRow {
    pub(super) conversation_id: String,
    pub(super) speech_output_override: String,
    pub(super) listening_pace_override: String,
    pub(super) policy_revision: i64,
    pub(super) updated_at: String,
}

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversation_voice_policies (
           conversation_id TEXT PRIMARY KEY,
           speech_output_override TEXT NOT NULL
             CHECK(speech_output_override IN ('inherit','muted')),
           listening_pace_override TEXT NOT NULL
             CHECK(listening_pace_override IN ('inherit','quick','balanced','patient')),
           policy_revision INTEGER NOT NULL CHECK(policy_revision >= 1),
           updated_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS conversation_voice_policy_events (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           runtime_run_id TEXT,
           tool_call_id TEXT,
           source_message_id TEXT,
           source TEXT NOT NULL CHECK(source IN ('tool','ui')),
           old_policy_json TEXT NOT NULL CHECK(json_valid(old_policy_json)),
           new_policy_json TEXT NOT NULL CHECK(json_valid(new_policy_json)),
           policy_revision INTEGER NOT NULL CHECK(policy_revision >= 1),
           result_code TEXT NOT NULL CHECK(result_code IN ('applied','unchanged','policy-conflict','invalid-input','failed')),
           created_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
           FOREIGN KEY(source_message_id) REFERENCES conversation_messages(id) ON DELETE SET NULL,
           UNIQUE(runtime_run_id, tool_call_id)
         );
         CREATE INDEX IF NOT EXISTS idx_voice_policy_events_conversation_created
           ON conversation_voice_policy_events(conversation_id, created_at DESC);",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO conversation_voice_policies(
           conversation_id,speech_output_override,listening_pace_override,policy_revision,updated_at
         ) SELECT id,'inherit','inherit',1,?1 FROM conversations WHERE task_mode='conversation'",
        [now_iso()],
    )?;
    Ok(())
}

pub(crate) fn policy_snapshot(
    state: &AppState,
    conversation_id: &str,
) -> Result<ConversationVoicePolicySnapshot, String> {
    validate_identifier(conversation_id, "conversation id")?;
    let (policy, voice) = state.sqlite_readers.read(|connection| {
        Ok((
            load_policy(connection, conversation_id)?,
            crate::persistence::load_voice_settings(connection)?,
        ))
    })?;
    Ok(snapshot_from(state.meeting.blocks_tts(), policy, voice))
}

pub(super) fn snapshot_from(
    meeting_blocked: bool,
    policy: PolicyRow,
    voice: VoiceRuntimeSettings,
) -> ConversationVoicePolicySnapshot {
    let presentation = effective_presentation_from(
        meeting_blocked,
        voice.auto_speak,
        None,
        &policy.speech_output_override,
    );
    let effective_listening_pace = match policy.listening_pace_override.as_str() {
        "inherit" => "balanced".to_string(),
        value => value.to_string(),
    };
    let effective_silence_timeout_ms = match policy.listening_pace_override.as_str() {
        "quick" => QUICK_SILENCE_TIMEOUT_MS,
        "balanced" => BALANCED_SILENCE_TIMEOUT_MS,
        "patient" => PATIENT_SILENCE_TIMEOUT_MS,
        _ => voice.silence_timeout_ms,
    };
    ConversationVoicePolicySnapshot {
        conversation_id: policy.conversation_id,
        speech_output: policy.speech_output_override,
        listening_pace: policy.listening_pace_override,
        policy_revision: policy.policy_revision,
        updated_at: policy.updated_at,
        effective_speech_output: presentation.decision,
        speech_reason_code: presentation.reason_code,
        effective_listening_pace,
        effective_silence_timeout_ms,
    }
}

pub(crate) fn update_policy_from_ui(
    state: &AppState,
    input: UpdateConversationVoicePolicyInput,
) -> Result<ConversationVoicePolicySnapshot, String> {
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.expected_revision < 1 {
        return Err("Voice policy revision must be positive".to_string());
    }
    if input.speech_output.is_none() && input.listening_pace.is_none() {
        return Err("At least one voice policy field must be updated".to_string());
    }
    if input
        .speech_output
        .as_deref()
        .is_some_and(|value| !matches!(value, "inherit" | "muted"))
        || input
            .listening_pace
            .as_deref()
            .is_some_and(|value| !matches!(value, "inherit" | "quick" | "balanced" | "patient"))
    {
        return Err("Voice policy value is invalid".to_string());
    }
    let (speech_changed, _) = apply_ui_policy(
        state,
        &input.conversation_id,
        input.expected_revision,
        input.speech_output.as_deref(),
        input.listening_pace.as_deref(),
    )?;
    if let (true, Some(speech_output)) = (speech_changed, input.speech_output.as_deref()) {
        super::apply_ui_speech_runtime(state, &input.conversation_id, speech_output)?;
    }
    policy_snapshot(state, &input.conversation_id)
}

pub(crate) fn reset_policy_from_ui(
    state: &AppState,
    input: ResetConversationVoicePolicyInput,
) -> Result<ConversationVoicePolicySnapshot, String> {
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.expected_revision < 1 {
        return Err("Voice policy revision must be positive".to_string());
    }
    let (speech_changed, _) = apply_ui_policy(
        state,
        &input.conversation_id,
        input.expected_revision,
        Some("inherit"),
        Some("inherit"),
    )?;
    if speech_changed {
        super::apply_ui_speech_runtime(state, &input.conversation_id, "inherit")?;
    }
    policy_snapshot(state, &input.conversation_id)
}

fn apply_ui_policy(
    state: &AppState,
    conversation_id: &str,
    expected_revision: i64,
    speech_output: Option<&str>,
    listening_pace: Option<&str>,
) -> Result<(bool, bool), String> {
    let _policy_guard = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    state.sqlite_writer.write(|connection| {
        ensure_policy(connection, conversation_id)?;
        let transaction = connection.transaction().map_err(database_error)?;
        let current = load_policy(&transaction, conversation_id)?;
        if current.policy_revision != expected_revision {
            return Err(
                "VOICE_POLICY_CONFLICT: The voice policy changed. Reload and try again."
                    .to_string(),
            );
        }
        let mut next = current.clone();
        if let Some(value) = speech_output {
            next.speech_output_override = value.to_string();
        }
        if let Some(value) = listening_pace {
            next.listening_pace_override = value.to_string();
        }
        let changed = next.speech_output_override != current.speech_output_override
            || next.listening_pace_override != current.listening_pace_override;
        if changed {
            next.policy_revision += 1;
            next.updated_at = now_iso();
            let updated = transaction
                .execute(
                    "UPDATE conversation_voice_policies
                 SET speech_output_override=?1, listening_pace_override=?2,
                     policy_revision=?3, updated_at=?4
                 WHERE conversation_id=?5 AND policy_revision=?6",
                    params![
                        next.speech_output_override,
                        next.listening_pace_override,
                        next.policy_revision,
                        next.updated_at,
                        conversation_id,
                        current.policy_revision
                    ],
                )
                .map_err(database_error)?;
            if updated != 1 {
                return Err(
                    "VOICE_POLICY_CONFLICT: The voice policy changed. Reload and try again."
                        .to_string(),
                );
            }
        }
        record_event(
            &transaction,
            conversation_id,
            None,
            None,
            None,
            "ui",
            &current,
            &next,
            if changed { "applied" } else { "unchanged" },
        )?;
        transaction.commit().map_err(database_error)?;
        Ok((
            next.speech_output_override != current.speech_output_override,
            next.listening_pace_override != current.listening_pace_override,
        ))
    })
}

pub(crate) fn ensure_policy(
    connection: &Connection,
    conversation_id: &str,
) -> Result<PolicyRow, String> {
    let conversation_mode = connection
        .query_row(
            "SELECT task_mode FROM conversations WHERE id=?1",
            params![conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| "Conversation does not exist".to_string())?;
    if conversation_mode != "conversation" {
        return Err("Voice behavior is available only for normal conversations".to_string());
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO conversation_voice_policies(
               conversation_id,speech_output_override,listening_pace_override,policy_revision,updated_at
             ) VALUES(?1,'inherit','inherit',1,?2)",
            params![conversation_id, now_iso()],
        )
        .map_err(database_error)?;
    load_policy(connection, conversation_id)
}

pub(super) fn load_policy(
    connection: &Connection,
    conversation_id: &str,
) -> Result<PolicyRow, String> {
    connection
        .query_row(
            "SELECT conversation_id,speech_output_override,listening_pace_override,
                    policy_revision,updated_at
             FROM conversation_voice_policies WHERE conversation_id=?1",
            params![conversation_id],
            |row| {
                Ok(PolicyRow {
                    conversation_id: row.get(0)?,
                    speech_output_override: row.get(1)?,
                    listening_pace_override: row.get(2)?,
                    policy_revision: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(database_error)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_event(
    connection: &Connection,
    conversation_id: &str,
    runtime_run_id: Option<&str>,
    tool_call_id: Option<&str>,
    source_message_id: Option<&str>,
    source: &str,
    old: &PolicyRow,
    new: &PolicyRow,
    result_code: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO conversation_voice_policy_events(
               id,conversation_id,runtime_run_id,tool_call_id,source_message_id,source,
               old_policy_json,new_policy_json,policy_revision,result_code,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                new_id("voice-policy-event"),
                conversation_id,
                runtime_run_id,
                tool_call_id,
                source_message_id,
                source,
                policy_json(old).to_string(),
                policy_json(new).to_string(),
                new.policy_revision,
                result_code,
                now_iso()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn policy_json(policy: &PolicyRow) -> Value {
    json!({
        "speechOutput": policy.speech_output_override,
        "listeningPace": policy.listening_pace_override,
        "policyRevision": policy.policy_revision,
    })
}

pub(super) fn source_message_id(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT input_message_id FROM runtime_runs WHERE id=?1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}
