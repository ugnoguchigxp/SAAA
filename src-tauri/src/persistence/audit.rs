use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::ipc::Channel;

use crate::voice::streaming_asr::contracts::VoiceAsrStreamEvent;
use crate::{database_error, new_id, now_iso, AppState, StartTurnInput};

const AUDIT_RETENTION_DAYS: i64 = 7;
const AUDIT_UI_EVENT_LIMIT: usize = 200;
const MILLISECONDS_PER_DAY: i64 = 86_400_000;
const MAX_ATTRIBUTES_JSON_BYTES: usize = 2_048;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum AuditAttributeValue {
    Boolean(bool),
    Integer(u64),
    Tag(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FrontendAuditEventInput {
    pub(crate) component: String,
    pub(crate) event_name: String,
    pub(crate) phase: String,
    #[serde(default)]
    pub(crate) outcome: Option<String>,
    #[serde(default)]
    pub(crate) correlation_id: Option<String>,
    #[serde(default)]
    pub(crate) causation_id: Option<String>,
    #[serde(default)]
    pub(crate) conversation_id: Option<String>,
    #[serde(default)]
    pub(crate) runtime_run_id: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) subject_id: Option<String>,
    #[serde(default)]
    pub(crate) failure_code: Option<String>,
    #[serde(default)]
    pub(crate) attributes: BTreeMap<String, AuditAttributeValue>,
}

pub(crate) struct VoiceAsrAuditChannel {
    channel: Channel<VoiceAsrStreamEvent>,
    audit: Option<VoiceAsrAuditContext>,
}

struct VoiceAsrAuditContext {
    connection: Arc<super::SqliteWriter>,
    conversation_id: String,
}

impl VoiceAsrAuditChannel {
    pub(crate) fn new(
        channel: Channel<VoiceAsrStreamEvent>,
        connection: Arc<super::SqliteWriter>,
        conversation_id: String,
    ) -> Self {
        Self {
            channel,
            audit: Some(VoiceAsrAuditContext {
                connection,
                conversation_id,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn plain(channel: Channel<VoiceAsrStreamEvent>) -> Self {
        Self {
            channel,
            audit: None,
        }
    }

    pub(crate) fn send(&self, event: VoiceAsrStreamEvent) -> tauri::Result<()> {
        if let Some(context) = &self.audit {
            context.record(&event);
        }
        self.channel.send(event)
    }
}

impl VoiceAsrAuditContext {
    fn record(&self, event: &VoiceAsrStreamEvent) {
        let Some((event_name, phase, outcome, subject_id, failure_code, attributes)) =
            voice_asr_event_fields(event)
        else {
            return;
        };
        let input = FrontendAuditEventInput {
            component: "voice-asr".to_string(),
            event_name: event_name.to_string(),
            phase: phase.to_string(),
            outcome: outcome.map(str::to_string),
            correlation_id: Some(session_id(event).to_string()),
            causation_id: None,
            conversation_id: Some(self.conversation_id.clone()),
            runtime_run_id: None,
            session_id: Some(session_id(event).to_string()),
            subject_id,
            failure_code,
            attributes,
        };
        let _ = self
            .connection
            .write(|connection| record_event(connection, &input));
    }
}

type VoiceAsrAuditFields = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<String>,
    Option<String>,
    BTreeMap<String, AuditAttributeValue>,
);

fn voice_asr_event_fields(event: &VoiceAsrStreamEvent) -> Option<VoiceAsrAuditFields> {
    let fields = match event {
        VoiceAsrStreamEvent::Ready {
            current_utterance_id,
            protocol,
            scope,
            ..
        } => (
            "asr-ready",
            "start",
            Some("success"),
            Some(current_utterance_id.clone()),
            None,
            tag_attributes([("protocol", *protocol), ("scope", *scope)]),
        ),
        VoiceAsrStreamEvent::Partial { .. } => return None,
        VoiceAsrStreamEvent::UtteranceDiscarded {
            utterance_id,
            reason,
            ..
        } => (
            "asr-utterance-discarded",
            "terminal",
            Some("cancelled"),
            Some(utterance_id.clone()),
            Some((*reason).to_string()),
            tag_attributes([("reasonCode", *reason)]),
        ),
        VoiceAsrStreamEvent::Final { utterance_id, .. } => (
            "asr-final-received",
            "terminal",
            Some("success"),
            Some(utterance_id.clone()),
            None,
            BTreeMap::new(),
        ),
        VoiceAsrStreamEvent::Failed {
            utterance_id,
            code,
            fatal,
            ..
        } => (
            "asr-failed",
            "error",
            Some("failure"),
            utterance_id.clone(),
            Some(wire_tag(code)),
            BTreeMap::from([("fatal".to_string(), AuditAttributeValue::Boolean(*fatal))]),
        ),
        VoiceAsrStreamEvent::Degraded {
            from,
            to,
            reason_code,
            ..
        } => (
            "asr-degraded",
            "state",
            Some("degraded"),
            None,
            Some(wire_tag(reason_code)),
            tag_attributes([("fromProtocol", *from), ("toProtocol", *to)]),
        ),
        VoiceAsrStreamEvent::Stopped { session_id } => (
            "asr-stopped",
            "terminal",
            Some("success"),
            Some(session_id.clone()),
            None,
            BTreeMap::new(),
        ),
    };
    Some(fields)
}

fn session_id(event: &VoiceAsrStreamEvent) -> &str {
    match event {
        VoiceAsrStreamEvent::Ready { session_id, .. }
        | VoiceAsrStreamEvent::Partial { session_id, .. }
        | VoiceAsrStreamEvent::UtteranceDiscarded { session_id, .. }
        | VoiceAsrStreamEvent::Final { session_id, .. }
        | VoiceAsrStreamEvent::Failed { session_id, .. }
        | VoiceAsrStreamEvent::Degraded { session_id, .. }
        | VoiceAsrStreamEvent::Stopped { session_id } => session_id,
    }
}

fn wire_tag<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn tag_attributes<const N: usize>(
    values: [(&str, &str); N],
) -> BTreeMap<String, AuditAttributeValue> {
    values
        .into_iter()
        .map(|(key, value)| (key.to_string(), AuditAttributeValue::Tag(value.to_string())))
        .collect()
}

pub(crate) fn initialize_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS audit_events (
          sequence INTEGER PRIMARY KEY AUTOINCREMENT,
          id TEXT NOT NULL UNIQUE CHECK(length(id) BETWEEN 1 AND 160 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
          occurred_at TEXT NOT NULL CHECK(length(occurred_at) BETWEEN 1 AND 32),
          component TEXT NOT NULL CHECK(component IN (
            'app','frontend','microphone','voice-asr','conversation','provider','tts',
            'meeting','settings','voice-policy','situation'
          )),
          event_name TEXT NOT NULL CHECK(length(event_name) BETWEEN 1 AND 80 AND event_name NOT GLOB '*[^a-z0-9-]*'),
          phase TEXT NOT NULL CHECK(phase IN ('request','start','state','progress','decision','terminal','error')),
          outcome TEXT CHECK(outcome IS NULL OR outcome IN (
            'success','failure','cancelled','interrupted','degraded','blocked'
          )),
          correlation_id TEXT CHECK(correlation_id IS NULL OR (length(correlation_id) BETWEEN 1 AND 160 AND correlation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          causation_id TEXT CHECK(causation_id IS NULL OR (length(causation_id) BETWEEN 1 AND 160 AND causation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          conversation_id TEXT CHECK(conversation_id IS NULL OR (length(conversation_id) BETWEEN 1 AND 160 AND conversation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          runtime_run_id TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          session_id TEXT CHECK(session_id IS NULL OR (length(session_id) BETWEEN 1 AND 160 AND session_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          subject_id TEXT CHECK(subject_id IS NULL OR (length(subject_id) BETWEEN 1 AND 160 AND subject_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          failure_code TEXT CHECK(failure_code IS NULL OR (length(failure_code) BETWEEN 1 AND 160 AND failure_code NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          attributes_json TEXT NOT NULL DEFAULT '{{}}'
            CHECK(length(attributes_json) <= {MAX_ATTRIBUTES_JSON_BYTES} AND json_valid(attributes_json) AND json_type(attributes_json) = 'object')
        );
        CREATE INDEX IF NOT EXISTS idx_audit_events_occurred ON audit_events(sequence DESC);
        CREATE INDEX IF NOT EXISTS idx_audit_events_retention
          ON audit_events(CAST(occurred_at AS INTEGER));
        CREATE INDEX IF NOT EXISTS idx_audit_events_correlation ON audit_events(correlation_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_audit_events_runtime_run ON audit_events(runtime_run_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_audit_events_session ON audit_events(session_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_audit_events_conversation ON audit_events(conversation_id, sequence);

        DROP TRIGGER IF EXISTS audit_events_prune_after_insert;

        CREATE TRIGGER IF NOT EXISTS audit_conversations_after_insert
        AFTER INSERT ON conversations
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,conversation_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.created_at,'conversation','conversation-created','terminal','success',
            NEW.id,NEW.id,NEW.id,json_object('taskMode',NEW.task_mode)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_conversation_messages_after_insert
        AFTER INSERT ON conversation_messages
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,causation_id,conversation_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.created_at,'conversation','message-persisted','terminal','success',
            NEW.conversation_id,NEW.id,NEW.conversation_id,NEW.id,json_object('role',NEW.role)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_runtime_runs_after_insert
        AFTER INSERT ON runtime_runs
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,correlation_id,conversation_id,runtime_run_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.started_at,
            CASE WHEN NEW.route_kind = 'voice.transcribe' THEN 'voice-asr'
                 WHEN NEW.route_kind = 'voice.speak' THEN 'tts'
                 ELSE 'conversation' END,
            'runtime-run-started','start',NEW.id,NEW.conversation_id,NEW.id,NEW.id,
            json_object('routeKind',NEW.route_kind,'providerId',NEW.provider_id,'state',NEW.status)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_runtime_runs_provider_after_update
        AFTER UPDATE OF provider_id ON runtime_runs
        WHEN NEW.provider_id IS NOT OLD.provider_id
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,correlation_id,conversation_id,runtime_run_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),strftime('%s','now') || '000',
            'provider','provider-selected','decision',NEW.id,NEW.conversation_id,NEW.id,NEW.provider_id,
            json_object('providerId',NEW.provider_id,'routeKind',NEW.route_kind)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_runtime_runs_status_after_update
        AFTER UPDATE OF status ON runtime_runs
        WHEN NEW.status IS NOT OLD.status
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,conversation_id,runtime_run_id,subject_id,failure_code,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),COALESCE(NEW.completed_at,strftime('%s','now') || '000'),
            CASE WHEN NEW.route_kind = 'voice.transcribe' THEN 'voice-asr'
                 WHEN NEW.route_kind = 'voice.speak' THEN 'tts'
                 ELSE 'conversation' END,
            'runtime-run-finished','terminal',
            CASE NEW.status WHEN 'completed' THEN 'success' WHEN 'cancelled' THEN 'cancelled'
                 WHEN 'interrupted' THEN 'interrupted' ELSE 'failure' END,
            NEW.id,NEW.conversation_id,NEW.id,NEW.id,NULL,
            json_object('previousState',OLD.status,'state',NEW.status,'routeKind',NEW.route_kind,'providerId',NEW.provider_id)
          );
        END;

        DROP TRIGGER IF EXISTS audit_provider_sessions_after_insert;
        CREATE TRIGGER audit_provider_sessions_after_insert
        AFTER INSERT ON provider_sessions
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,correlation_id,runtime_run_id,session_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.started_at,'provider','provider-session-started','start',
            NEW.id,NEW.runtime_run_id,NEW.id,NEW.provider_id,json_object('providerId',NEW.provider_id,'state',NEW.status)
          );
        END;

        DROP TRIGGER IF EXISTS audit_provider_sessions_after_update;
        CREATE TRIGGER audit_provider_sessions_after_update
        AFTER UPDATE OF status ON provider_sessions
        WHEN NEW.status IS NOT OLD.status
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.updated_at,'provider','provider-session-state','state',
            CASE NEW.status WHEN 'completed' THEN 'success' WHEN 'cancelled' THEN 'cancelled'
                 WHEN 'interrupted' THEN 'interrupted' WHEN 'failed' THEN 'failure' ELSE NULL END,
            NEW.id,NEW.runtime_run_id,NEW.id,NEW.provider_id,NULL,
            json_object('previousState',OLD.status,'state',NEW.status,'providerId',NEW.provider_id)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_meeting_sessions_after_insert
        AFTER INSERT ON meeting_sessions
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,correlation_id,session_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.started_at,'meeting','meeting-session-started','start',
            NEW.id,NEW.id,NEW.id,json_object('state',NEW.status,'providerId',NEW.stt_provider_id,
                                             'microphoneEnabled',NEW.microphone_enabled,
                                             'systemAudioEnabled',NEW.system_audio_enabled)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_meeting_sessions_after_update
        AFTER UPDATE OF status,error_code ON meeting_sessions
        WHEN NEW.status IS NOT OLD.status OR NEW.error_code IS NOT OLD.error_code
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,session_id,subject_id,failure_code,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),COALESCE(NEW.ended_at,NEW.saved_at,strftime('%s','now') || '000'),
            'meeting','meeting-session-state','state',
            CASE NEW.status WHEN 'completed' THEN 'success' WHEN 'saved' THEN 'success'
                 WHEN 'discarded' THEN 'cancelled' WHEN 'interrupted' THEN 'interrupted'
                 WHEN 'failed' THEN 'failure' ELSE NULL END,
            NEW.id,NEW.id,NEW.id,NEW.error_code,json_object('previousState',OLD.status,'state',NEW.status)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_meeting_transcript_entries_after_insert
        AFTER INSERT ON meeting_transcript_entries
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,session_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.created_at,'meeting','transcript-segment-persisted','terminal','success',
            NEW.session_id,NEW.session_id,NEW.id,
            json_object('lane',NEW.lane,'sequence',NEW.sequence)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_settings_documents_after_insert
        AFTER INSERT ON settings_documents
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.updated_at,'settings','settings-document-created','terminal','success',
            NEW.namespace || ':' || NEW.key,NEW.namespace || ':' || NEW.key,
            json_object('namespace',NEW.namespace,'settingsKey',NEW.key,'schemaVersion',NEW.schema_version)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_settings_documents_after_update
        AFTER UPDATE ON settings_documents
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.updated_at,'settings','settings-document-updated','terminal','success',
            NEW.namespace || ':' || NEW.key,NEW.namespace || ':' || NEW.key,
            json_object('namespace',NEW.namespace,'settingsKey',NEW.key,'schemaVersion',NEW.schema_version)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_voice_policy_events_after_insert
        AFTER INSERT ON conversation_voice_policy_events
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,causation_id,conversation_id,runtime_run_id,subject_id,failure_code,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.created_at,'voice-policy','voice-policy-changed','decision',
            CASE WHEN NEW.result_code = 'applied' THEN 'success' ELSE 'blocked' END,
            COALESCE(NEW.runtime_run_id,NEW.conversation_id),COALESCE(NEW.tool_call_id,NEW.source_message_id),
            NEW.conversation_id,NEW.runtime_run_id,NEW.id,NEW.result_code,
            json_object('source',NEW.source,'policyRevision',NEW.policy_revision,'resultCode',NEW.result_code)
          );
        END;

        CREATE TRIGGER IF NOT EXISTS audit_situation_ledger_after_insert
        AFTER INSERT ON situation_ledger
        BEGIN
          INSERT INTO audit_events(
            id,occurred_at,component,event_name,phase,outcome,correlation_id,subject_id,attributes_json
          ) VALUES(
            'audit_' || lower(hex(randomblob(16))),NEW.observed_at,'situation','situation-evaluated','decision','success',
            NEW.id,NEW.id,json_object('entryKind',NEW.entry_kind,'proposedAttention',NEW.proposed_attention,
                                      'actualExecution',NEW.actual_execution,'actualPresentation',NEW.actual_presentation)
          );
        END;
        "#
    ))?;
    connection.execute(
        "UPDATE audit_events
         SET runtime_run_id=(
           SELECT session.runtime_run_id FROM provider_sessions AS session
           WHERE session.id=audit_events.session_id
         )
         WHERE component='provider'
           AND event_name IN ('provider-session-started','provider-session-state')
           AND runtime_run_id IS NULL
           AND session_id IS NOT NULL",
        [],
    )?;
    let now_ms = now_iso().parse::<i64>().unwrap_or_default();
    prune_expired_events(connection, now_ms)?;
    Ok(())
}

fn prune_expired_events(connection: &Connection, now_ms: i64) -> rusqlite::Result<usize> {
    let cutoff_ms = now_ms.saturating_sub(AUDIT_RETENTION_DAYS * MILLISECONDS_PER_DAY);
    connection.execute(
        "DELETE FROM audit_events WHERE CAST(occurred_at AS INTEGER) < ?1",
        [cutoff_ms],
    )
}

pub(crate) fn record_frontend_event(
    state: &AppState,
    input: &FrontendAuditEventInput,
) -> Result<(), String> {
    state
        .sqlite_writer
        .write(|connection| record_event(connection, input))
}

pub(crate) fn record_voice_asr_command(
    state: &AppState,
    event_name: &str,
    session_id: &str,
    conversation_id: Option<&str>,
    outcome: Option<&str>,
    failure: Option<&str>,
    attributes: BTreeMap<String, AuditAttributeValue>,
) {
    let event = FrontendAuditEventInput {
        component: "voice-asr".to_string(),
        event_name: event_name.to_string(),
        phase: if outcome.is_some() {
            "terminal"
        } else {
            "request"
        }
        .to_string(),
        outcome: outcome.map(str::to_string),
        correlation_id: Some(session_id.to_string()),
        causation_id: None,
        conversation_id: conversation_id.map(str::to_string),
        runtime_run_id: None,
        session_id: Some(session_id.to_string()),
        subject_id: Some(session_id.to_string()),
        failure_code: failure.map(|failure| {
            if validate_tag(failure).is_ok() {
                failure.to_string()
            } else {
                "internal-error".to_string()
            }
        }),
        attributes,
    };
    let _ = record_frontend_event(state, &event);
}

pub(crate) fn record_turn_request(state: &AppState, input: &StartTurnInput) -> Result<(), String> {
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "inputOrigin".to_string(),
        AuditAttributeValue::Tag(input.input_origin.clone()),
    );
    attributes.insert(
        "presentationMode".to_string(),
        AuditAttributeValue::Tag(input.presentation_mode.clone()),
    );
    let event = FrontendAuditEventInput {
        component: "conversation".to_string(),
        event_name: "turn-requested".to_string(),
        phase: "request".to_string(),
        outcome: None,
        correlation_id: Some(input.run_id.clone()),
        causation_id: input.source_id.clone(),
        conversation_id: Some(input.conversation_id.clone()),
        runtime_run_id: Some(input.run_id.clone()),
        session_id: None,
        subject_id: Some(input.run_id.clone()),
        failure_code: None,
        attributes,
    };
    state
        .sqlite_writer
        .write(|connection| record_event(connection, &event))
}

fn record_event(connection: &Connection, input: &FrontendAuditEventInput) -> Result<(), String> {
    validate_event(input)?;
    let attributes_json = serde_json::to_string(&input.attributes)
        .map_err(|error| format!("Could not encode audit attributes: {error}"))?;
    if attributes_json.len() > MAX_ATTRIBUTES_JSON_BYTES {
        return Err("Audit attributes are too large".to_string());
    }
    connection
        .execute(
            "INSERT INTO audit_events(
               id,occurred_at,component,event_name,phase,outcome,correlation_id,causation_id,
               conversation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                new_id("audit"),
                now_iso(),
                input.component,
                input.event_name,
                input.phase,
                input.outcome,
                input.correlation_id,
                input.causation_id,
                input.conversation_id,
                input.runtime_run_id,
                input.session_id,
                input.subject_id,
                input.failure_code,
                attributes_json,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn validate_event(input: &FrontendAuditEventInput) -> Result<(), String> {
    if !matches!(
        input.component.as_str(),
        "app"
            | "frontend"
            | "microphone"
            | "voice-asr"
            | "conversation"
            | "provider"
            | "tts"
            | "meeting"
            | "settings"
            | "voice-policy"
            | "situation"
    ) {
        return Err("Invalid audit component".to_string());
    }
    if !is_event_name(&input.event_name) {
        return Err("Invalid audit event name".to_string());
    }
    if !matches!(
        input.phase.as_str(),
        "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error"
    ) {
        return Err("Invalid audit phase".to_string());
    }
    if input.outcome.as_deref().is_some_and(|outcome| {
        !matches!(
            outcome,
            "success" | "failure" | "cancelled" | "interrupted" | "degraded" | "blocked"
        )
    }) {
        return Err("Invalid audit outcome".to_string());
    }
    for value in [
        input.correlation_id.as_deref(),
        input.causation_id.as_deref(),
        input.conversation_id.as_deref(),
        input.runtime_run_id.as_deref(),
        input.session_id.as_deref(),
        input.subject_id.as_deref(),
        input.failure_code.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_tag(value)?;
    }
    for (key, value) in &input.attributes {
        if !is_allowed_attribute(key) {
            return Err(format!("Unsupported audit attribute: {key}"));
        }
        if let AuditAttributeValue::Tag(value) = value {
            validate_tag(value)?;
        }
    }
    Ok(())
}

fn is_event_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn validate_tag(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err("Invalid audit identifier".to_string());
    }
    Ok(())
}

fn is_allowed_attribute(key: &str) -> bool {
    matches!(
        key,
        "state"
            | "previousState"
            | "nextState"
            | "reasonCode"
            | "providerId"
            | "providerKind"
            | "routeKind"
            | "routeId"
            | "protocol"
            | "scope"
            | "sequence"
            | "enabled"
            | "inputOrigin"
            | "presentationMode"
            | "finalizeCurrent"
            | "fallbackUsed"
            | "selectionReason"
            | "releaseStatus"
            | "source"
            | "policyRevision"
            | "resultCode"
            | "recoverExisting"
            | "commitReason"
            | "fatal"
            | "fromProtocol"
            | "toProtocol"
            | "queueDepth"
            | "deliveryMode"
            | "microphoneEnabled"
            | "systemAudioEnabled"
            | "taskMode"
            | "role"
            | "lane"
            | "namespace"
            | "settingsKey"
            | "schemaVersion"
            | "entryKind"
            | "proposedAttention"
            | "actualExecution"
            | "actualPresentation"
    )
}

pub(crate) fn recent_events(connection: &Connection, limit: usize) -> Result<Vec<Value>, String> {
    let limit = limit.clamp(1, 2_000) as i64;
    let mut statement = connection
        .prepare(
            "SELECT sequence,id,occurred_at,component,event_name,phase,outcome,correlation_id,
                    causation_id,conversation_id,runtime_run_id,session_id,subject_id,failure_code,attributes_json
             FROM (
               SELECT audit.sequence,audit.id,audit.occurred_at,audit.component,audit.event_name,
                      audit.phase,audit.outcome,audit.correlation_id,audit.causation_id,
                      audit.conversation_id,audit.runtime_run_id,audit.session_id,audit.subject_id,
                      COALESCE(
                        audit.failure_code,
                        CASE WHEN audit.event_name='runtime-run-finished' THEN (
                          SELECT run.failure_code FROM runtime_runs AS run WHERE run.id=audit.runtime_run_id
                        ) END,
                        CASE WHEN audit.event_name='provider-session-state' THEN (
                          SELECT COALESCE(provider.failure_kind,provider.release_failure_kind)
                          FROM provider_sessions AS provider WHERE provider.id=audit.session_id
                        ) END
                      ) AS failure_code,
                      audit.attributes_json
               FROM audit_events AS audit ORDER BY audit.sequence DESC LIMIT ?1
             ) ORDER BY sequence ASC",
        )
        .map_err(database_error)?;
    let events = statement
        .query_map([limit], |row| {
            let attributes_json: String = row.get(14)?;
            let attributes =
                serde_json::from_str::<Value>(&attributes_json).unwrap_or_else(|_| json!({}));
            Ok(json!({
                "sequence": row.get::<_, i64>(0)?,
                "id": row.get::<_, String>(1)?,
                "occurredAt": row.get::<_, String>(2)?,
                "component": row.get::<_, String>(3)?,
                "eventName": row.get::<_, String>(4)?,
                "phase": row.get::<_, String>(5)?,
                "outcome": row.get::<_, Option<String>>(6)?,
                "correlationId": row.get::<_, Option<String>>(7)?,
                "causationId": row.get::<_, Option<String>>(8)?,
                "conversationId": row.get::<_, Option<String>>(9)?,
                "runtimeRunId": row.get::<_, Option<String>>(10)?,
                "sessionId": row.get::<_, Option<String>>(11)?,
                "subjectId": row.get::<_, Option<String>>(12)?,
                "failureCode": row.get::<_, Option<String>>(13)?,
                "attributes": attributes
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(events)
}

pub(crate) fn list_ui_events(state: &AppState) -> Result<Vec<Value>, String> {
    state.sqlite_readers.read(|connection| {
        let mut events = recent_events(connection, AUDIT_UI_EVENT_LIMIT)?;
        events.reverse();
        Ok(events)
    })
}

#[tauri::command]
pub(crate) fn list_audit_events(state: tauri::State<'_, AppState>) -> Result<Vec<Value>, String> {
    list_ui_events(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_triggers_record_relations_without_message_content() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('message_audit','conversation_primary','user','secret transcript','100')",
                [],
            )
            .expect("message inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
                 VALUES('run_audit','conversation_primary','conversation.respond','running','101')",
                [],
            )
            .expect("run inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id,provider_id,runtime_run_id,provider_kind,status,started_at,updated_at
                 ) VALUES(
                   'provider_session_audit','provider_audit','run_audit',
                   'openai-compatible','running','101','101'
                 )",
                [],
            )
            .expect("provider session inserts");
        connection
            .execute(
                "UPDATE provider_sessions SET status='completed',updated_at='102'
                 WHERE id='provider_session_audit'",
                [],
            )
            .expect("provider session finishes");
        connection
            .execute(
                "UPDATE runtime_runs SET status='failed',failure_code='request-timeout',
                 error_message='secret provider detail',completed_at='102' WHERE id='run_audit'",
                [],
            )
            .expect("run finishes");

        let encoded = serde_json::to_string(&recent_events(&connection, 100).expect("events load"))
            .expect("events encode");
        assert!(encoded.contains("message-persisted"));
        assert!(encoded.contains("runtime-run-finished"));
        assert!(encoded.contains("request-timeout"));
        assert!(encoded.contains("run_audit"));
        assert!(!encoded.contains("secret transcript"));
        assert!(!encoded.contains("secret provider detail"));
        let related_provider_events: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events
                 WHERE component='provider' AND runtime_run_id='run_audit'",
                [],
                |row| row.get(0),
            )
            .expect("provider audit relations load");
        assert_eq!(related_provider_events, 2);
    }

    #[test]
    fn frontend_audit_rejects_freeform_content() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let input = FrontendAuditEventInput {
            component: "microphone".to_string(),
            event_name: "capture-failed".to_string(),
            phase: "error".to_string(),
            outcome: Some("failure".to_string()),
            correlation_id: Some("voice_session".to_string()),
            causation_id: None,
            conversation_id: None,
            runtime_run_id: None,
            session_id: None,
            subject_id: None,
            failure_code: Some("permission-denied".to_string()),
            attributes: BTreeMap::from([(
                "reasonCode".to_string(),
                AuditAttributeValue::Tag("raw transcript with spaces".to_string()),
            )]),
        };
        assert!(record_event(&connection, &input).is_err());
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE event_name='capture-failed'",
                [],
                |row| row.get(0),
            )
            .expect("audit count loads");
        assert_eq!(count, 0);
    }

    #[test]
    fn voice_asr_channel_audits_metadata_without_transcript_text() {
        let connection = Arc::new(crate::persistence::SqliteWriter::from_connection(
            Connection::open_in_memory().expect("database opens"),
        ));
        crate::initialize_database(&connection.lock().expect("database lock"))
            .expect("database initializes");
        let channel = Channel::new(|_| Ok(()));
        let audited = VoiceAsrAuditChannel::new(
            channel,
            connection.clone(),
            crate::PRIMARY_CONVERSATION_ID.to_string(),
        );
        audited
            .send(VoiceAsrStreamEvent::Partial {
                session_id: "session_audit".to_string(),
                utterance_id: "utterance_audit".to_string(),
                revision: 3,
                start_ms: 100,
                end_ms: 700,
                stable_text: "secret partial".to_string(),
                unstable_text: "secret tail".to_string(),
                language: Some("ja".to_string()),
            })
            .expect("partial sends");
        audited
            .send(VoiceAsrStreamEvent::Final {
                session_id: "session_audit".to_string(),
                utterance_id: "utterance_audit".to_string(),
                revision: 4,
                start_ms: 100,
                end_ms: 900,
                text: "secret recognized speech".to_string(),
                language: Some("ja".to_string()),
            })
            .expect("event sends");

        let connection = connection.lock().expect("database lock");
        let encoded = serde_json::to_string(&recent_events(&connection, 20).expect("events load"))
            .expect("events encode");
        assert!(encoded.contains("asr-final-received"));
        assert!(!encoded.contains("asr-partial-received"));
        assert!(encoded.contains("utterance_audit"));
        assert!(!encoded.contains("durationMs"));
        assert!(!encoded.contains("revision"));
        assert!(!encoded.contains("secret recognized speech"));
        assert!(!encoded.contains("\"ja\""));
    }

    #[test]
    fn turn_request_links_voice_utterance_to_runtime_run() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let input = StartTurnInput {
            run_id: "run_voice_audit".to_string(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            content: "never persist this prompt in audit".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: Some("utterance_voice_audit".to_string()),
            input_origin: "voice".to_string(),
            presentation_mode: "visual".to_string(),
        };
        record_turn_request(&state, &input).expect("turn request audits");

        let connection = state.sqlite_writer.lock().expect("database lock");
        let event = recent_events(&connection, 1)
            .expect("events load")
            .pop()
            .expect("event exists");
        assert_eq!(event["correlationId"], "run_voice_audit");
        assert_eq!(event["causationId"], "utterance_voice_audit");
        assert_eq!(event["runtimeRunId"], "run_voice_audit");
        assert!(!event.to_string().contains("never persist this prompt"));
    }

    #[test]
    fn startup_prunes_only_audit_events_older_than_seven_days() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let now_ms = now_iso().parse::<i64>().expect("timestamp is milliseconds");
        let eight_days_ago = now_ms - (8 * MILLISECONDS_PER_DAY);
        let six_days_ago = now_ms - (6 * MILLISECONDS_PER_DAY);
        connection
            .execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                 VALUES('audit_expired_probe',?1,'app','retention-probe','terminal','success','{}'),
                       ('audit_recent_probe',?2,'app','retention-probe','terminal','success','{}')",
                params![eight_days_ago.to_string(), six_days_ago.to_string()],
            )
            .expect("retention probes insert");

        let before_restart: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_expired_probe'",
                [],
                |row| row.get(0),
            )
            .expect("pre-startup count loads");
        assert_eq!(before_restart, 1);

        crate::initialize_database(&connection).expect("startup cleanup runs");

        let expired: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_expired_probe'",
                [],
                |row| row.get(0),
            )
            .expect("expired count loads");
        let recent: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE id='audit_recent_probe'",
                [],
                |row| row.get(0),
            )
            .expect("recent count loads");
        assert_eq!(expired, 0);
        assert_eq!(recent, 1);
    }

    #[test]
    fn retention_keeps_events_at_the_seven_day_boundary() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let now_ms = 10 * MILLISECONDS_PER_DAY;
        let cutoff_ms = now_ms - (AUDIT_RETENTION_DAYS * MILLISECONDS_PER_DAY);
        connection
            .execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                 VALUES('audit_before_cutoff',?1,'app','retention-probe','terminal','success','{}'),
                       ('audit_at_cutoff',?2,'app','retention-probe','terminal','success','{}')",
                params![(cutoff_ms - 1).to_string(), cutoff_ms.to_string()],
            )
            .expect("boundary probes insert");

        prune_expired_events(&connection, now_ms).expect("retention cleanup runs");

        let ids = recent_events(&connection, 20)
            .expect("events load")
            .into_iter()
            .filter_map(|event| event["id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(!ids.iter().any(|id| id == "audit_before_cutoff"));
        assert!(ids.iter().any(|id| id == "audit_at_cutoff"));
    }

    #[test]
    fn ui_events_are_bounded_and_newest_first() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        for index in 0..205 {
            connection
                .execute(
                    "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
                     VALUES(?1,?2,'app','ui-probe','terminal','success','{}')",
                    params![format!("audit_ui_{index}"), index.to_string()],
                )
                .expect("UI audit fixture inserts");
        }
        let state = crate::test_support::app_state(connection);

        let events = list_ui_events(&state).expect("UI events load");

        assert_eq!(events.len(), AUDIT_UI_EVENT_LIMIT);
        assert_eq!(
            events.first().and_then(|event| event["id"].as_str()),
            Some("audit_ui_204")
        );
        assert_eq!(
            events.last().and_then(|event| event["id"].as_str()),
            Some("audit_ui_5")
        );
    }
}
