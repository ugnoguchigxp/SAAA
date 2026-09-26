use crate::persistence::SqliteWriter;
use crate::voice::streaming_asr::contracts::VoiceAsrStreamEvent;
use crate::{database_error, new_id, now_iso, AppState, StartTurnInput};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::ipc::Channel;
#[path = "audit/audit_event_sort_field.rs"]
mod audit_event_sort_field;
#[path = "audit/record_event.rs"]
pub(crate) mod record_event;
#[path = "audit/voice_frontend_observer.rs"]
mod voice_frontend_observer;
pub(crate) use audit_event_sort_field::{
    initialize_schema, record_frontend_event, record_turn_request, record_voice_asr_command,
    AuditAttributeValue, AuditEventListInput, AuditEventSortField, FrontendAuditEventInput,
    SortDirection, VoiceAsrAuditChannel,
};
use audit_event_sort_field::{
    prune_expired_events, session_id, AUDIT_RETENTION_DAYS, AUDIT_UI_EVENT_LIMIT,
    MAX_ATTRIBUTES_JSON_BYTES, MILLISECONDS_PER_DAY,
};
pub(crate) use record_event::{
    list_audit_events, list_ui_events, recent_events, record_tool_execution,
};
use record_event::{record_event, validate_tag};
#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;
