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
pub(crate) use audit_event_sort_field::{AuditEventSortField, SortDirection, AuditEventListInput, AuditAttributeValue, FrontendAuditEventInput, VoiceAsrAuditChannel, initialize_schema, record_frontend_event, record_voice_asr_command, record_turn_request};
use audit_event_sort_field::{AUDIT_RETENTION_DAYS, AUDIT_UI_EVENT_LIMIT, MILLISECONDS_PER_DAY, MAX_ATTRIBUTES_JSON_BYTES, session_id, prune_expired_events};
pub(crate) use record_event::{recent_events, list_ui_events, list_audit_events, record_tool_execution};
use record_event::{record_event, validate_tag};
#[cfg(test)]
#[path = "audit/tests.rs"]
mod tests;
