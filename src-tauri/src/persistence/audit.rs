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
pub(crate) use audit_event_sort_field::{
    initialize_schema, record_frontend_event, record_turn_request, AuditAttributeValue,
    AuditEventListInput, AuditEventSortField, FrontendAuditEventInput, SortDirection,
};
#[cfg(any(test, feature = "offline-contracts"))]
use audit_event_sort_field::{
    prune_expired_events, session_id, AUDIT_RETENTION_DAYS, MILLISECONDS_PER_DAY,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use audit_event_sort_field::{record_voice_asr_command, VoiceAsrAuditChannel};
use audit_event_sort_field::{AUDIT_UI_EVENT_LIMIT, MAX_ATTRIBUTES_JSON_BYTES};
pub(crate) use record_event::recent_events;
use record_event::record_event;
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use record_event::record_tool_execution;
#[cfg(any(test, feature = "offline-contracts"))]
use record_event::validate_tag;
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use record_event::{list_audit_events, list_ui_events};
#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;
