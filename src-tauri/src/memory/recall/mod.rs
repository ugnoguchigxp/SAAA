#[cfg(any(test, feature = "offline-contracts"))]
use super::contracts::{
    RecallConversationInput, RecallConversationOutput, RecallError, RecallErrorCode,
    RecallTimeFilter, RecallTimePreset, MAX_RECALL_CALLS_PER_TURN, RECALL_NOTICE,
    RECALL_RETRIEVAL_MODE,
};
use chrono::{DateTime, TimeZone, Utc};
#[cfg(any(test, feature = "offline-contracts"))]
use chrono::{Datelike, Duration, LocalResult, Months, NaiveDate, Weekday};
#[cfg(any(test, feature = "offline-contracts"))]
use chrono_tz::Tz;
#[cfg(any(test, feature = "offline-contracts"))]
use rusqlite::{params, Connection, OptionalExtension};
mod search;
#[cfg(any(test, feature = "offline-contracts"))]
use search::*;
mod recall_execution_context;
mod resolve_time;
pub use recall_execution_context::{
    execute, migrate_v9_to_v10, record_failed_attempt, remaining_calls, system_timezone,
    RecallExecutionContext,
};
use recall_execution_context::{
    Candidate, CursorState, InternalEvent, InternalWindow, ResolvedRange, CURSOR_TTL_MS,
    MAX_ABSOLUTE_RANGE_DAYS, MAX_EVENTS_PER_WINDOW, MAX_MERGED_EVENTS_PER_WINDOW,
    MAX_NEIGHBOR_TURNS, MAX_OUTPUT_TOKEN_BUDGET, MAX_QUERY_TERMS,
};
#[cfg(any(test, feature = "offline-contracts"))]
use resolve_time::start_of_day;
use resolve_time::{filter_digest, load_cursor, resolve_time, store_cursor};
#[cfg(test)]
mod tests;
