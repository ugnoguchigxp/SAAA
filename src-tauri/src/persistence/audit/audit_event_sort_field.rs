use super::*;
pub(super) const AUDIT_RETENTION_DAYS: i64 = 7;
pub(super) const AUDIT_UI_EVENT_LIMIT: usize = 200;
pub(super) const MILLISECONDS_PER_DAY: i64 = 86_400_000;
pub(super) const MAX_ATTRIBUTES_JSON_BYTES: usize = 2_048;
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AuditEventSortField {
    OccurredAt,
    Component,
    EventName,
    Phase,
    Outcome,
    FailureCode,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AuditEventListInput {
    pub(super) sort_by: AuditEventSortField,
    pub(super) direction: SortDirection,
}
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
    pub(super) channel: Channel<VoiceAsrStreamEvent>,
    pub(super) audit: Option<VoiceAsrAuditContext>,
}
struct VoiceAsrAuditContext {
    pub(super) connection: Arc<super::SqliteWriter>,
    pub(super) conversation_id: String,
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
pub(super) fn voice_asr_event_fields(event: &VoiceAsrStreamEvent) -> Option<VoiceAsrAuditFields> {
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
pub(super) fn session_id(event: &VoiceAsrStreamEvent) -> &str {
    match event {
        VoiceAsrStreamEvent::Ready { session_id, .. }
        | VoiceAsrStreamEvent::Partial { session_id, .. }
        | VoiceAsrStreamEvent::UtteranceDiscarded { session_id, .. }
        | VoiceAsrStreamEvent::Final { session_id, .. }
        | VoiceAsrStreamEvent::Failed { session_id, .. }
        | VoiceAsrStreamEvent::Stopped { session_id } => session_id,
    }
}
pub(super) fn wire_tag<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}
pub(super) fn tag_attributes<const N: usize>(
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
pub(super) fn prune_expired_events(
    connection: &Connection,
    now_ms: i64,
) -> rusqlite::Result<usize> {
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
