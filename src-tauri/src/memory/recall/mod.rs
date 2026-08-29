use chrono::{
    DateTime, Datelike, Duration, LocalResult, Months, NaiveDate, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};

use super::contracts::{
    RecallConversationInput, RecallConversationOutput, RecallError, RecallErrorCode,
    RecallTimeFilter, RecallTimePreset, MAX_RECALL_CALLS_PER_TURN, RECALL_NOTICE,
    RECALL_RETRIEVAL_MODE,
};

const MAX_QUERY_CHARS: usize = 256;
const MAX_QUERY_TERMS: usize = 8;
const MAX_ABSOLUTE_RANGE_DAYS: i64 = 366;
const MAX_WINDOWS: usize = 3;
const MAX_NEIGHBOR_TURNS: i64 = 2;
const MAX_EVENTS_PER_WINDOW: usize = 32;
const MAX_MERGED_EVENTS_PER_WINDOW: usize = 64;
// UTF-8 bytes are a provider-independent upper bound on subword-token count.
const MAX_OUTPUT_TOKEN_BUDGET: usize = 3_000;
const CANDIDATE_BATCH_SIZE: usize = 5;
const MAX_CANDIDATE_SCAN: usize = CANDIDATE_BATCH_SIZE;
const CURSOR_TTL_MS: i64 = 24 * 60 * 60 * 1_000;

pub struct RecallExecutionContext<'a> {
    pub runtime_run_id: &'a str,
    pub tool_call_id: &'a str,
    pub now: DateTime<Utc>,
    pub timezone: Tz,
}

#[derive(Debug, Clone)]
struct ResolvedRange {
    from_ms: i64,
    to_exclusive_ms: i64,
    timezone: Tz,
    label: String,
}

#[derive(Debug, Clone)]
struct CursorState {
    offset: usize,
    range: Option<ResolvedRange>,
    snapshot_max_rowid: i64,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    conversation_id: String,
    score: f64,
}

#[derive(Debug, Clone)]
struct InternalEvent {
    id: String,
    conversation_id: String,
    role: String,
    content: String,
    created_at_ms: i64,
    rowid: i64,
    turn_sequence: i64,
}

#[derive(Debug, Clone)]
struct InternalWindow {
    conversation_id: String,
    first_turn: i64,
    last_turn: i64,
    score: f64,
    matched_event_refs: Vec<String>,
    events: Vec<InternalEvent>,
}

pub fn system_timezone() -> Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::UTC)
}

pub fn remaining_calls(
    connection: &Connection,
    runtime_run_id: &str,
) -> Result<usize, RecallError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_recall_attempts WHERE runtime_run_id=?1",
            [runtime_run_id],
            |row| row.get(0),
        )
        .map_err(local_unavailable)?;
    Ok(MAX_RECALL_CALLS_PER_TURN.saturating_sub(usize::try_from(count).unwrap_or(usize::MAX)))
}

pub fn record_failed_attempt(
    connection: &mut Connection,
    context: &RecallExecutionContext<'_>,
) -> Result<(), RecallError> {
    reserve_attempt(connection, context).map(|_| ())
}

pub fn execute(
    connection: &mut Connection,
    context: RecallExecutionContext<'_>,
    input: RecallConversationInput,
) -> Result<RecallConversationOutput, RecallError> {
    let (current_message_id, call_index) = reserve_attempt(connection, &context)?;

    let normalized_query = normalize_query(input.query.as_deref())?;
    if normalized_query.is_none() && input.time.is_none() {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "At least one of query or time is required.",
        ));
    }
    validate_cursor(input.cursor.as_deref())?;
    let filter_digest = filter_digest(normalized_query.as_deref(), input.time.as_ref());
    let cursor_state = if let Some(cursor) = input.cursor.as_deref() {
        load_cursor(
            connection,
            context.runtime_run_id,
            cursor,
            &filter_digest,
            context.now.timestamp_millis(),
        )?
    } else {
        let snapshot_max_rowid = connection
            .query_row(
                "SELECT COALESCE(MAX(rowid),0) FROM conversation_messages",
                [],
                |row| row.get(0),
            )
            .map_err(local_unavailable)?;
        CursorState {
            offset: 0,
            range: input
                .time
                .as_ref()
                .map(|filter| resolve_time(filter, context.now, context.timezone))
                .transpose()?,
            snapshot_max_rowid,
        }
    };

    let mut windows = Vec::<InternalWindow>::new();
    let mut offset = cursor_state.offset;
    let mut scanned = 0_usize;
    let mut has_more = false;
    'scan: while scanned < MAX_CANDIDATE_SCAN {
        let candidates = search_candidates(
            connection,
            normalized_query.as_deref(),
            cursor_state.range.as_ref(),
            &current_message_id,
            cursor_state.snapshot_max_rowid,
            offset,
            CANDIDATE_BATCH_SIZE,
        )?;
        if candidates.is_empty() {
            has_more = false;
            break;
        }
        let batch_len = candidates.len();
        let mut consumed = 0_usize;
        for candidate in candidates {
            let candidate_window = load_window(
                connection,
                &candidate,
                cursor_state.range.as_ref(),
                &current_message_id,
                cursor_state.snapshot_max_rowid,
            )?;
            if let Some(existing) = windows
                .iter_mut()
                .find(|window| windows_overlap(window, &candidate_window))
            {
                merge_windows(existing, candidate_window);
            } else if windows.len() == MAX_WINDOWS {
                offset = offset.saturating_add(consumed);
                has_more = true;
                break 'scan;
            } else {
                windows.push(candidate_window);
            }
            consumed += 1;
            scanned += 1;
            if scanned == MAX_CANDIDATE_SCAN {
                offset = offset.saturating_add(consumed);
                has_more = !search_candidates(
                    connection,
                    normalized_query.as_deref(),
                    cursor_state.range.as_ref(),
                    &current_message_id,
                    cursor_state.snapshot_max_rowid,
                    offset,
                    1,
                )?
                .is_empty();
                break 'scan;
            }
        }
        offset = offset.saturating_add(consumed);
        if consumed < batch_len {
            has_more = true;
            break;
        }
        if batch_len < CANDIDATE_BATCH_SIZE {
            has_more = false;
            break;
        }
        has_more = true;
    }

    let next_cursor = if has_more {
        Some(store_cursor(
            connection,
            context.runtime_run_id,
            &filter_digest,
            offset,
            cursor_state.range.as_ref(),
            cursor_state.snapshot_max_rowid,
            context.now.timestamp_millis(),
        )?)
    } else {
        None
    };

    let (projected_windows, budget_truncated) = project_windows(
        windows,
        cursor_state
            .range
            .as_ref()
            .map(|range| range.timezone)
            .unwrap_or(context.timezone),
    );
    let reason_code = if projected_windows.is_empty() {
        "continuity-no-hit"
    } else {
        "ok"
    };
    let output = RecallConversationOutput {
        notice: RECALL_NOTICE,
        resolved_time_range: cursor_state.range.as_ref().map(public_range),
        windows: projected_windows,
        truncated: has_more || budget_truncated,
        next_cursor,
        reason_code,
        retrieval_mode: RECALL_RETRIEVAL_MODE,
    };
    persist_receipt(
        connection,
        &context,
        normalized_query.as_deref(),
        cursor_state.range.as_ref(),
        call_index,
        &output,
    )?;
    Ok(output)
}

fn reserve_attempt(
    connection: &mut Connection,
    context: &RecallExecutionContext<'_>,
) -> Result<(String, i64), RecallError> {
    if context.tool_call_id.is_empty()
        || context.tool_call_id.len() > 160
        || !context
            .tool_call_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "Tool call identifier is invalid.",
        ));
    }
    let transaction = connection.transaction().map_err(local_unavailable)?;
    let current_message_id = transaction
        .query_row(
            "SELECT input_message_id FROM runtime_runs WHERE id=?1 AND status='running'",
            [context.runtime_run_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(local_unavailable)?
        .flatten()
        .ok_or_else(|| {
            RecallError::new(
                RecallErrorCode::LocalRecallUnavailable,
                "The active turn is unavailable for conversation recall.",
            )
        })?;
    let duplicate: bool = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM conversation_recall_attempts
               WHERE runtime_run_id=?1 AND tool_call_id=?2
             )",
            params![context.runtime_run_id, context.tool_call_id],
            |row| row.get(0),
        )
        .map_err(local_unavailable)?;
    let call_index: i64 = transaction
        .query_row(
            "SELECT COUNT(*) + 1 FROM conversation_recall_attempts WHERE runtime_run_id=?1",
            [context.runtime_run_id],
            |row| row.get(0),
        )
        .map_err(local_unavailable)?;
    if call_index > i64::try_from(MAX_RECALL_CALLS_PER_TURN).unwrap_or(3) {
        return Err(RecallError::new(
            RecallErrorCode::CallLimitExceeded,
            "Conversation recall is limited to three calls per turn.",
        ));
    }
    transaction
        .execute(
            "INSERT INTO conversation_recall_attempts(
               id,runtime_run_id,tool_call_id,call_index,created_at_ms
             ) VALUES(?1,?2,?3,?4,?5)",
            params![
                format!("recall_attempt_{}", uuid::Uuid::new_v4().simple()),
                context.runtime_run_id,
                context.tool_call_id,
                call_index,
                context.now.timestamp_millis(),
            ],
        )
        .map_err(local_unavailable)?;
    transaction.commit().map_err(local_unavailable)?;
    if duplicate {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "Tool call has already been processed.",
        ));
    }
    Ok((current_message_id, call_index))
}

pub fn migrate_v9_to_v10(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 10 {
        return Ok(());
    }
    let fts_exists: bool = connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_master WHERE type='table' AND name='conversation_messages_fts'
         )",
        [],
        |row| row.get(0),
    )?;
    let recall_trigger_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type='trigger' AND name IN (
           'conversation_messages_recall_insert',
           'conversation_messages_recall_update',
           'conversation_messages_recall_delete'
         )",
        [],
        |row| row.get(0),
    )?;
    let input_message_column_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name='input_message_id')",
        [],
        |row| row.get(0),
    )?;
    if !input_message_column_exists {
        connection.execute_batch("ALTER TABLE runtime_runs ADD COLUMN input_message_id TEXT;")?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_conversation_messages_global_created
           ON conversation_messages(CAST(created_at AS INTEGER), id);
         CREATE VIRTUAL TABLE IF NOT EXISTS conversation_messages_fts USING fts5(
           message_id UNINDEXED,
           content,
           tokenize='trigram'
         );
         CREATE TRIGGER IF NOT EXISTS conversation_messages_recall_insert
         AFTER INSERT ON conversation_messages
         WHEN NEW.role IN ('user','assistant','transcript')
         BEGIN
           INSERT INTO conversation_messages_fts(message_id,content) VALUES(NEW.id,NEW.content);
         END;
         CREATE TRIGGER IF NOT EXISTS conversation_messages_recall_update
         AFTER UPDATE OF content,role ON conversation_messages
         BEGIN
           DELETE FROM conversation_messages_fts WHERE message_id=OLD.id;
           INSERT INTO conversation_messages_fts(message_id,content)
             SELECT NEW.id,NEW.content WHERE NEW.role IN ('user','assistant','transcript');
         END;
         CREATE TRIGGER IF NOT EXISTS conversation_messages_recall_delete
         AFTER DELETE ON conversation_messages
         BEGIN
           DELETE FROM conversation_messages_fts WHERE message_id=OLD.id;
         END;
         CREATE TABLE IF NOT EXISTS conversation_recall_cursors (
           id TEXT PRIMARY KEY,
           runtime_run_id TEXT NOT NULL,
           filter_digest TEXT NOT NULL CHECK(length(filter_digest)=64),
           candidate_offset INTEGER NOT NULL CHECK(candidate_offset >= 0),
           range_from_ms INTEGER,
           range_to_exclusive_ms INTEGER,
           timezone TEXT,
           range_label TEXT,
           expires_at_ms INTEGER NOT NULL,
           created_at_ms INTEGER NOT NULL,
           snapshot_max_rowid INTEGER NOT NULL CHECK(snapshot_max_rowid >= 0),
           FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE CASCADE,
           CHECK((range_from_ms IS NULL) = (range_to_exclusive_ms IS NULL)),
           CHECK((range_from_ms IS NULL) = (timezone IS NULL)),
           CHECK((range_from_ms IS NULL) = (range_label IS NULL))
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_recall_cursors_expiry
           ON conversation_recall_cursors(expires_at_ms);
         CREATE TABLE IF NOT EXISTS conversation_recall_attempts (
           id TEXT PRIMARY KEY,
           runtime_run_id TEXT NOT NULL,
           tool_call_id TEXT NOT NULL CHECK(length(tool_call_id) BETWEEN 1 AND 160),
           call_index INTEGER NOT NULL CHECK(call_index BETWEEN 1 AND 3),
           created_at_ms INTEGER NOT NULL,
           FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE CASCADE,
           UNIQUE(runtime_run_id,call_index)
         );
         CREATE TABLE IF NOT EXISTS conversation_recall_receipts (
           id TEXT PRIMARY KEY,
           runtime_run_id TEXT NOT NULL,
           tool_call_id TEXT NOT NULL CHECK(length(tool_call_id) BETWEEN 1 AND 160),
           call_index INTEGER NOT NULL CHECK(call_index BETWEEN 1 AND 3),
           query_digest TEXT CHECK(query_digest IS NULL OR length(query_digest)=64),
           range_from_ms INTEGER,
           range_to_exclusive_ms INTEGER,
           timezone TEXT,
           matched_event_refs_json TEXT NOT NULL CHECK(length(matched_event_refs_json) <= 4096),
           reason_code TEXT NOT NULL CHECK(reason_code IN ('ok','continuity-no-hit')),
           created_at_ms INTEGER NOT NULL,
           FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE CASCADE,
           UNIQUE(runtime_run_id,tool_call_id),
           UNIQUE(runtime_run_id,call_index)
         );",
    )?;
    let attempts_schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master
         WHERE type='table' AND name='conversation_recall_attempts'",
        [],
        |row| row.get(0),
    )?;
    let compact_attempts_schema = attempts_schema.split_whitespace().collect::<String>();
    if compact_attempts_schema.contains("UNIQUE(runtime_run_id,tool_call_id)") {
        connection.execute_batch(
            "CREATE TABLE conversation_recall_attempts_v10_repair (
               id TEXT PRIMARY KEY,
               runtime_run_id TEXT NOT NULL,
               tool_call_id TEXT NOT NULL CHECK(length(tool_call_id) BETWEEN 1 AND 160),
               call_index INTEGER NOT NULL CHECK(call_index BETWEEN 1 AND 3),
               created_at_ms INTEGER NOT NULL,
               FOREIGN KEY(runtime_run_id) REFERENCES runtime_runs(id) ON DELETE CASCADE,
               UNIQUE(runtime_run_id,call_index)
             );
             INSERT INTO conversation_recall_attempts_v10_repair(
               id,runtime_run_id,tool_call_id,call_index,created_at_ms
             )
             SELECT id,runtime_run_id,tool_call_id,call_index,created_at_ms
             FROM conversation_recall_attempts;
             DROP TABLE conversation_recall_attempts;
             ALTER TABLE conversation_recall_attempts_v10_repair
             RENAME TO conversation_recall_attempts;",
        )?;
    }
    let snapshot_column_exists: bool = connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM pragma_table_info('conversation_recall_cursors')
           WHERE name='snapshot_max_rowid'
         )",
        [],
        |row| row.get(0),
    )?;
    if !snapshot_column_exists {
        connection.execute("DELETE FROM conversation_recall_cursors", [])?;
        connection.execute_batch(
            "ALTER TABLE conversation_recall_cursors
             ADD COLUMN snapshot_max_rowid INTEGER NOT NULL DEFAULT 0
             CHECK(snapshot_max_rowid >= 0);",
        )?;
    }
    if version < 10 || !fts_exists || recall_trigger_count != 3 {
        connection.execute("DELETE FROM conversation_messages_fts", [])?;
        connection.execute(
            "INSERT INTO conversation_messages_fts(message_id,content)
             SELECT id,content FROM conversation_messages
             WHERE role IN ('user','assistant','transcript')",
            [],
        )?;
    }
    Ok(())
}

fn normalize_query(query: Option<&str>) -> Result<Option<String>, RecallError> {
    let Some(query) = query else {
        return Ok(None);
    };
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = normalized.chars().count();
    if count == 0 || count > MAX_QUERY_CHARS {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "Query must contain between one and 256 characters.",
        ));
    }
    if normalized.contains('\0') {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "Query contains an invalid character.",
        ));
    }
    Ok(Some(normalized))
}

fn validate_cursor(cursor: Option<&str>) -> Result<(), RecallError> {
    if cursor.is_some_and(|value| {
        value.is_empty()
            || value.len() > 160
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    }) {
        return Err(RecallError::new(
            RecallErrorCode::InvalidInput,
            "Cursor is invalid.",
        ));
    }
    Ok(())
}

fn resolve_time(
    filter: &RecallTimeFilter,
    now: DateTime<Utc>,
    timezone: Tz,
) -> Result<ResolvedRange, RecallError> {
    match filter {
        RecallTimeFilter::Absolute { from, to_exclusive } => {
            let from = DateTime::parse_from_rfc3339(from)
                .map_err(|_| invalid_time())?
                .with_timezone(&Utc);
            let to = DateTime::parse_from_rfc3339(to_exclusive)
                .map_err(|_| invalid_time())?
                .with_timezone(&Utc);
            let from_ms = from.timestamp_millis();
            let to_exclusive_ms = to.timestamp_millis();
            if from >= to
                || from_ms >= to_exclusive_ms
                || to - from > Duration::days(MAX_ABSOLUTE_RANGE_DAYS)
            {
                return Err(invalid_time());
            }
            Ok(ResolvedRange {
                from_ms,
                to_exclusive_ms,
                timezone,
                label: "absolute".to_string(),
            })
        }
        RecallTimeFilter::Preset { preset } => {
            let local_now = now.with_timezone(&timezone);
            let today = local_now.date_naive();
            let (from, to) = match preset {
                RecallTimePreset::Today => (start_of_day(timezone, today)?, now),
                RecallTimePreset::Yesterday => {
                    let from_date = today - Duration::days(1);
                    (
                        start_of_day(timezone, from_date)?,
                        start_of_day(timezone, today)?,
                    )
                }
                RecallTimePreset::DayBeforeYesterday => {
                    let from_date = today - Duration::days(2);
                    let to_date = today - Duration::days(1);
                    (
                        start_of_day(timezone, from_date)?,
                        start_of_day(timezone, to_date)?,
                    )
                }
                RecallTimePreset::CurrentWeek => {
                    let monday = previous_or_same_monday(today);
                    (start_of_day(timezone, monday)?, now)
                }
                RecallTimePreset::PreviousCalendarWeek => {
                    let current_monday = previous_or_same_monday(today);
                    (
                        start_of_day(timezone, current_monday - Duration::days(7))?,
                        start_of_day(timezone, current_monday)?,
                    )
                }
                RecallTimePreset::Past7Days => (now - Duration::days(7), now),
                RecallTimePreset::PreviousCalendarMonth => {
                    let current_month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                        .ok_or_else(invalid_time)?;
                    let previous_month = current_month
                        .checked_sub_months(Months::new(1))
                        .ok_or_else(invalid_time)?;
                    (
                        start_of_day(timezone, previous_month)?,
                        start_of_day(timezone, current_month)?,
                    )
                }
            };
            Ok(ResolvedRange {
                from_ms: from.timestamp_millis(),
                to_exclusive_ms: to.timestamp_millis(),
                timezone,
                label: preset.as_str().to_string(),
            })
        }
    }
}

fn start_of_day(timezone: Tz, date: NaiveDate) -> Result<DateTime<Utc>, RecallError> {
    let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(invalid_time)?;
    for minute in 0..1_440_i64 {
        let candidate = midnight + Duration::minutes(minute);
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    Err(invalid_time())
}

fn previous_or_same_monday(date: NaiveDate) -> NaiveDate {
    let days = match date.weekday() {
        Weekday::Mon => 0,
        weekday => i64::from(weekday.num_days_from_monday()),
    };
    date - Duration::days(days)
}

fn invalid_time() -> RecallError {
    RecallError::new(
        RecallErrorCode::InvalidTimeRange,
        "Time range is invalid or exceeds 366 days.",
    )
}

fn filter_digest(query: Option<&str>, time: Option<&RecallTimeFilter>) -> String {
    let time_key = match time {
        None => "none".to_string(),
        Some(RecallTimeFilter::Preset { preset }) => format!("preset:{}", preset.as_str()),
        Some(RecallTimeFilter::Absolute { from, to_exclusive }) => {
            format!("absolute:{from}:{to_exclusive}")
        }
    };
    digest(&format!("{}\n{time_key}", query.unwrap_or_default()))
}

fn load_cursor(
    connection: &Connection,
    runtime_run_id: &str,
    cursor: &str,
    expected_filter_digest: &str,
    now_ms: i64,
) -> Result<CursorState, RecallError> {
    let row = connection
        .query_row(
            "SELECT filter_digest,candidate_offset,range_from_ms,range_to_exclusive_ms,
                    timezone,range_label,expires_at_ms,snapshot_max_rowid
             FROM conversation_recall_cursors
             WHERE id=?1 AND runtime_run_id=?2",
            params![cursor, runtime_run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(local_unavailable)?
        .ok_or_else(|| {
            RecallError::new(
                RecallErrorCode::CursorFilterMismatch,
                "Cursor is missing or does not belong to this turn.",
            )
        })?;
    if row.0 != expected_filter_digest || row.6 <= now_ms {
        return Err(RecallError::new(
            RecallErrorCode::CursorFilterMismatch,
            "Cursor does not match the requested filters or has expired.",
        ));
    }
    let range = match (row.2, row.3, row.4, row.5) {
        (Some(from_ms), Some(to_exclusive_ms), Some(timezone), Some(label)) => {
            if from_ms >= to_exclusive_ms
                || DateTime::<Utc>::from_timestamp_millis(from_ms).is_none()
                || DateTime::<Utc>::from_timestamp_millis(to_exclusive_ms).is_none()
            {
                return Err(local_unavailable(""));
            }
            let timezone = timezone.parse::<Tz>().map_err(|_| local_unavailable(""))?;
            Some(ResolvedRange {
                from_ms,
                to_exclusive_ms,
                timezone,
                label,
            })
        }
        (None, None, None, None) => None,
        _ => return Err(local_unavailable("")),
    };
    if row.7 < 0 {
        return Err(local_unavailable(""));
    }
    Ok(CursorState {
        offset: usize::try_from(row.1).map_err(|_| local_unavailable(""))?,
        range,
        snapshot_max_rowid: row.7,
    })
}

fn store_cursor(
    connection: &Connection,
    runtime_run_id: &str,
    filter_digest: &str,
    offset: usize,
    range: Option<&ResolvedRange>,
    snapshot_max_rowid: i64,
    now_ms: i64,
) -> Result<String, RecallError> {
    let id = format!("recall_cursor_{}", uuid::Uuid::new_v4().simple());
    connection
        .execute(
            "INSERT INTO conversation_recall_cursors(
               id,runtime_run_id,filter_digest,candidate_offset,range_from_ms,
               range_to_exclusive_ms,timezone,range_label,expires_at_ms,created_at_ms,
               snapshot_max_rowid
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                runtime_run_id,
                filter_digest,
                i64::try_from(offset).map_err(|_| local_unavailable(""))?,
                range.map(|value| value.from_ms),
                range.map(|value| value.to_exclusive_ms),
                range.map(|value| value.timezone.name()),
                range.map(|value| value.label.as_str()),
                now_ms.saturating_add(CURSOR_TTL_MS),
                now_ms,
                snapshot_max_rowid,
            ],
        )
        .map_err(local_unavailable)?;
    connection
        .execute(
            "DELETE FROM conversation_recall_cursors WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map_err(local_unavailable)?;
    Ok(id)
}

mod search;
use search::*;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::TimeZone as _;

    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(
                   id TEXT PRIMARY KEY,title TEXT,task_mode TEXT,created_at TEXT,updated_at TEXT
                 );
                 CREATE TABLE conversation_messages(
                   id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,role TEXT NOT NULL,
                   content TEXT NOT NULL,created_at TEXT NOT NULL
                 );
                 CREATE TABLE runtime_runs(
                   id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL,status TEXT NOT NULL,
                   input_message_id TEXT
                 );",
            )
            .expect("base schema creates");
        migrate_v9_to_v10(&connection).expect("recall schema migrates");
        connection
    }

    fn insert_turn(
        connection: &Connection,
        conversation_id: &str,
        sequence: usize,
        created_at_ms: i64,
        user: &str,
        assistant: &str,
    ) {
        connection
            .execute(
                "INSERT OR IGNORE INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES(?1,'conversation','0','0')",
                [conversation_id],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES(?1,?2,'user',?3,?4)",
                params![
                    format!("message_{sequence}_user"),
                    conversation_id,
                    user,
                    created_at_ms.to_string()
                ],
            )
            .expect("user inserts");
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES(?1,?2,'assistant',?3,?4)",
                params![
                    format!("message_{sequence}_assistant"),
                    conversation_id,
                    assistant,
                    created_at_ms.saturating_add(1).to_string()
                ],
            )
            .expect("assistant inserts");
    }

    fn start_run(connection: &Connection, current_message_id: &str) {
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,status,input_message_id)
                 VALUES('run_recall','current','running',?1)",
                [current_message_id],
            )
            .expect("run inserts");
    }

    #[test]
    fn relative_time_presets_use_calendar_boundaries_and_rolling_days() {
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let timezone = chrono_tz::Asia::Tokyo;
        let yesterday = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Yesterday,
            },
            now,
            timezone,
        )
        .expect("yesterday resolves");
        assert_eq!(public_range(&yesterday).from, "2026-08-28T00:00:00+09:00");
        assert_eq!(
            public_range(&yesterday).to_exclusive,
            "2026-08-29T00:00:00+09:00"
        );
        let previous_week = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::PreviousCalendarWeek,
            },
            now,
            timezone,
        )
        .expect("previous week resolves");
        assert_eq!(
            public_range(&previous_week).from,
            "2026-08-17T00:00:00+09:00"
        );
        assert_eq!(
            public_range(&previous_week).to_exclusive,
            "2026-08-24T00:00:00+09:00"
        );
        let rolling = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Past7Days,
            },
            now,
            timezone,
        )
        .expect("rolling range resolves");
        assert_ne!(rolling.from_ms, previous_week.from_ms);

        let leap_month = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::PreviousCalendarMonth,
            },
            Utc.with_ymd_and_hms(2024, 3, 1, 3, 0, 0)
                .single()
                .expect("leap fixture time"),
            timezone,
        )
        .expect("leap month resolves");
        assert_eq!(public_range(&leap_month).from, "2024-02-01T00:00:00+09:00");
        assert_eq!(
            public_range(&leap_month).to_exclusive,
            "2024-03-01T00:00:00+09:00"
        );

        let dst_timezone = chrono_tz::America::New_York;
        let dst_yesterday = resolve_time(
            &RecallTimeFilter::Preset {
                preset: RecallTimePreset::Yesterday,
            },
            Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0)
                .single()
                .expect("DST fixture time"),
            dst_timezone,
        )
        .expect("DST day resolves");
        assert_eq!(
            dst_yesterday.to_exclusive_ms - dst_yesterday.from_ms,
            Duration::hours(23).num_milliseconds()
        );

        assert!(start_of_day(
            chrono_tz::Pacific::Apia,
            NaiveDate::from_ymd_opt(2011, 12, 30).expect("skipped date fixture")
        )
        .is_err());
    }

    #[test]
    fn projected_relevance_increases_with_stronger_sqlite_bm25_matches() {
        assert!(score(-2.0) > score(-1.0));
        assert!(score(-1.0) > score(0.0));
        assert_eq!(score(f64::NAN), 0.0);
    }

    #[test]
    fn version_ten_schema_is_repaired_without_rebuilding_the_database() {
        let connection = database();
        insert_turn(
            &connection,
            "repair_history",
            1,
            1_000,
            "repair keyword",
            "repair answer",
        );
        connection
            .execute_batch(
                "PRAGMA user_version=10;
                 DELETE FROM conversation_messages_fts;
                 DROP TABLE conversation_recall_attempts;
                 CREATE TABLE conversation_recall_attempts (
                   id TEXT PRIMARY KEY,
                   runtime_run_id TEXT NOT NULL,
                   tool_call_id TEXT NOT NULL,
                   call_index INTEGER NOT NULL,
                   created_at_ms INTEGER NOT NULL,
                   UNIQUE(runtime_run_id,tool_call_id),
                   UNIQUE(runtime_run_id,call_index)
                 );
                 DROP TRIGGER conversation_messages_recall_insert;
                 DROP TABLE conversation_recall_cursors;
                 CREATE TABLE conversation_recall_cursors (
                   id TEXT PRIMARY KEY,
                   runtime_run_id TEXT NOT NULL,
                   filter_digest TEXT NOT NULL,
                   candidate_offset INTEGER NOT NULL,
                   range_from_ms INTEGER,
                   range_to_exclusive_ms INTEGER,
                   timezone TEXT,
                   range_label TEXT,
                   expires_at_ms INTEGER NOT NULL,
                   created_at_ms INTEGER NOT NULL
                 );",
            )
            .expect("old v10 fixture creates");

        migrate_v9_to_v10(&connection).expect("v10 schema repairs");
        let attempts: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='table' AND name='conversation_recall_attempts'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("attempt table reads");
        let insert_trigger: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='trigger' AND name='conversation_messages_recall_insert'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("trigger reads");
        let snapshot_column: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('conversation_recall_cursors')
                   WHERE name='snapshot_max_rowid'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("cursor columns read");
        let attempts_schema: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type='table' AND name='conversation_recall_attempts'",
                [],
                |row| row.get(0),
            )
            .expect("attempt schema reads");
        let fts_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages_fts",
                [],
                |row| row.get(0),
            )
            .expect("fts rows read");
        assert!(attempts);
        assert!(insert_trigger);
        assert!(snapshot_column);
        assert_eq!(fts_rows, 2);
        assert!(!attempts_schema
            .split_whitespace()
            .collect::<String>()
            .contains("UNIQUE(runtime_run_id,tool_call_id)"));
    }

    #[test]
    fn keyword_and_time_search_returns_bounded_neighbor_turns_without_current_message() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let yesterday_ms = Utc
            .with_ymd_and_hms(2026, 8, 28, 1, 0, 0)
            .single()
            .expect("yesterday time")
            .timestamp_millis();
        for sequence in 0..6 {
            insert_turn(
                &connection,
                "history",
                sequence,
                yesterday_ms + i64::try_from(sequence).expect("sequence") * 10,
                if sequence == 3 {
                    "SQLiteの保存方針"
                } else {
                    "別の話題"
                },
                &format!("回答{sequence}"),
            );
        }
        insert_turn(
            &connection,
            "current",
            99,
            now.timestamp_millis(),
            "昨日SQLiteの保存方針について何を話した？",
            "未回答",
        );
        start_run(&connection, "message_99_user");
        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_1",
                now,
                timezone,
            },
            RecallConversationInput {
                query: Some("SQLite 保存方針".to_string()),
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: None,
            },
        )
        .expect("recall succeeds");
        assert_eq!(output.reason_code, "ok");
        assert_eq!(output.windows.len(), 1);
        let events = &output.windows[0].events;
        assert!(events.iter().any(|event| event.content.contains("SQLite")));
        assert!(!events
            .iter()
            .any(|event| event.event_ref == "message_99_user"));
        assert!(events.len() <= 10);
        let raw_result_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE content LIKE '%retrievalMode%'",
                [],
                |row| row.get(0),
            )
            .expect("count reads");
        assert_eq!(raw_result_count, 0);
        let receipt_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_recall_receipts",
                [],
                |row| row.get(0),
            )
            .expect("receipt count reads");
        assert_eq!(receipt_count, 1);
    }

    #[test]
    fn raw_windows_exclude_system_messages_and_bound_dense_turns() {
        let mut connection = database();
        insert_turn(
            &connection,
            "history",
            1,
            1_000,
            "dense-anchor-key",
            "first answer",
        );
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('old-system','history','system','obsolete hidden instruction','1002')",
                [],
            )
            .expect("system message inserts");
        for index in 0..100 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,'history','transcript',?2,?3)",
                    params![
                        format!("dense_transcript_{index}"),
                        format!("transcript {index}"),
                        (1_003 + index).to_string()
                    ],
                )
                .expect("dense transcript inserts");
        }
        insert_turn(&connection, "current", 2, 2_000, "current", "answer");
        start_run(&connection, "message_2_user");
        let snapshot_max_rowid: i64 = connection
            .query_row("SELECT MAX(rowid) FROM conversation_messages", [], |row| {
                row.get(0)
            })
            .expect("snapshot reads");
        let candidate = search_candidates(
            &connection,
            Some("dense-anchor-key"),
            None,
            "message_2_user",
            snapshot_max_rowid,
            0,
            1,
        )
        .expect("candidate search succeeds")
        .pop()
        .expect("anchor exists");
        let window = load_window(
            &connection,
            &candidate,
            None,
            "message_2_user",
            snapshot_max_rowid,
        )
        .expect("window loads");
        assert!(window.events.len() <= MAX_EVENTS_PER_WINDOW);
        assert!(window.events.iter().any(|event| event.id == candidate.id));
        assert!(window.events.iter().all(|event| event.role != "system"));

        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_dense",
                now: Utc
                    .timestamp_millis_opt(3_000)
                    .single()
                    .expect("fixture time"),
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("dense-anchor-key".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("bounded recall succeeds");
        assert!(!output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content.contains("obsolete hidden instruction")));
    }

    #[test]
    fn projection_preserves_every_anchor_within_the_shared_token_budget() {
        let events = (0..5)
            .map(|index| InternalEvent {
                id: format!("anchor_{index}"),
                conversation_id: "history".to_string(),
                role: "user".to_string(),
                content: "長".repeat(2_000),
                created_at_ms: i64::from(index),
                rowid: i64::from(index),
                turn_sequence: i64::from(index),
            })
            .collect::<Vec<_>>();
        let (windows, truncated) = project_windows(
            vec![InternalWindow {
                conversation_id: "history".to_string(),
                first_turn: 0,
                last_turn: 4,
                score: 1.0,
                matched_event_refs: events.iter().map(|event| event.id.clone()).collect(),
                events,
            }],
            chrono_tz::UTC,
        );

        assert!(truncated);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].matched_event_refs.len(), 5);
        assert_eq!(windows[0].events.len(), 5);
        assert!(windows[0]
            .events
            .iter()
            .all(|event| event.truncated && !event.content.is_empty()));
        assert!(
            windows[0]
                .events
                .iter()
                .map(|event| event.content.len())
                .sum::<usize>()
                <= MAX_OUTPUT_TOKEN_BUDGET
        );
    }

    #[test]
    fn cursor_does_not_drop_an_unprocessed_hit_inside_a_previous_window() {
        let mut connection = database();
        insert_turn(&connection, "history", 1, 1_000, "opening", "answer");
        for index in 0..6 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES(?1,'history','transcript',?2,?3)",
                    params![
                        format!("overlap_{index}"),
                        format!("overlap-key {index}"),
                        (1_002 + index).to_string()
                    ],
                )
                .expect("overlapping hit inserts");
        }
        insert_turn(&connection, "current", 99, 2_000, "current", "answer");
        start_run(&connection, "message_99_user");
        let now = Utc
            .timestamp_millis_opt(3_000)
            .single()
            .expect("fixture time");

        let first = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_overlap_1",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("overlap-key".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("first page succeeds");
        assert_eq!(first.windows[0].matched_event_refs.len(), 5);
        let second = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_overlap_2",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("overlap-key".to_string()),
                time: None,
                cursor: first.next_cursor,
            },
        )
        .expect("second page succeeds");

        assert_eq!(second.windows.len(), 1);
        assert_eq!(
            second.windows[0].matched_event_refs,
            vec![opaque_ref("event", "overlap_0")]
        );
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn query_metacharacters_remain_plain_text_and_never_become_sql_or_fts_syntax() {
        let mut connection = database();
        insert_turn(
            &connection,
            "history",
            1,
            1_000,
            "SQLite ordinary discussion",
            "ordinary answer",
        );
        insert_turn(&connection, "current", 2, 2_000, "current", "answer");
        start_run(&connection, "message_2_user");
        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_metacharacters",
                now: Utc
                    .timestamp_millis_opt(3_000)
                    .single()
                    .expect("fixture time"),
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("SQLite\" OR *; DROP TABLE conversation_messages --".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect("metacharacters are handled as bounded plain text");
        assert_eq!(output.reason_code, "continuity-no-hit");
        let messages_table_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type='table' AND name='conversation_messages'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("schema reads");
        assert!(messages_table_exists);
    }

    #[test]
    fn absolute_ranges_and_call_limits_are_enforced() {
        let mut connection = database();
        insert_turn(&connection, "current", 1, 1_000, "current", "answer");
        start_run(&connection, "message_1_user");
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 0, 0, 0)
            .single()
            .expect("fixture time");
        let invalid = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_invalid",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: None,
                time: Some(RecallTimeFilter::Absolute {
                    from: "2025-01-01T00:00:00Z".to_string(),
                    to_exclusive: "2026-08-29T00:00:00Z".to_string(),
                }),
                cursor: None,
            },
        )
        .expect_err("oversized range is rejected");
        assert_eq!(invalid.code, RecallErrorCode::InvalidTimeRange);

        for index in 0..2 {
            execute(
                &mut connection,
                RecallExecutionContext {
                    runtime_run_id: "run_recall",
                    tool_call_id: &format!("call_{index}"),
                    now,
                    timezone: chrono_tz::UTC,
                },
                RecallConversationInput {
                    query: Some("missing".to_string()),
                    time: None,
                    cursor: None,
                },
            )
            .expect("bounded no-hit call succeeds");
        }
        let limit = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_3",
                now,
                timezone: chrono_tz::UTC,
            },
            RecallConversationInput {
                query: Some("missing".to_string()),
                time: None,
                cursor: None,
            },
        )
        .expect_err("fourth call is rejected");
        assert_eq!(limit.code, RecallErrorCode::CallLimitExceeded);
        let (attempts, receipts): (i64, i64) = (
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_recall_attempts",
                    [],
                    |row| row.get(0),
                )
                .expect("attempt count reads"),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_recall_receipts",
                    [],
                    |row| row.get(0),
                )
                .expect("receipt count reads"),
        );
        assert_eq!(attempts, 3, "invalid attempts count toward the hard cap");
        assert_eq!(receipts, 2, "only completed searches create receipts");

        let sub_millisecond = resolve_time(
            &RecallTimeFilter::Absolute {
                from: "2026-08-29T00:00:00.0001Z".to_string(),
                to_exclusive: "2026-08-29T00:00:00.0002Z".to_string(),
            },
            now,
            chrono_tz::UTC,
        )
        .expect_err("a range empty at storage precision is rejected");
        assert_eq!(sub_millisecond.code, RecallErrorCode::InvalidTimeRange);
    }

    #[test]
    fn duplicate_tool_call_ids_still_consume_the_turn_call_limit() {
        let mut connection = database();
        insert_turn(&connection, "current", 1, 1_000, "current", "answer");
        start_run(&connection, "message_1_user");
        let now = Utc
            .timestamp_millis_opt(2_000)
            .single()
            .expect("fixture time");
        let input = RecallConversationInput {
            query: Some("missing".to_string()),
            time: None,
            cursor: None,
        };
        execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_duplicate",
                now,
                timezone: chrono_tz::UTC,
            },
            input.clone(),
        )
        .expect("first call succeeds");
        for _ in 0..2 {
            let duplicate = execute(
                &mut connection,
                RecallExecutionContext {
                    runtime_run_id: "run_recall",
                    tool_call_id: "call_duplicate",
                    now,
                    timezone: chrono_tz::UTC,
                },
                input.clone(),
            )
            .expect_err("duplicate call is rejected");
            assert_eq!(duplicate.code, RecallErrorCode::InvalidInput);
        }
        assert_eq!(
            remaining_calls(&connection, "run_recall").expect("remaining calls read"),
            0
        );
        let limit = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_after_duplicates",
                now,
                timezone: chrono_tz::UTC,
            },
            input,
        )
        .expect_err("fourth invocation is rejected");
        assert_eq!(limit.code, RecallErrorCode::CallLimitExceeded);
    }

    #[test]
    fn time_only_search_resolves_yesterday_in_the_supplied_timezone() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 29, 3, 30, 0)
            .single()
            .expect("fixture time");
        let yesterday = Utc
            .with_ymd_and_hms(2026, 8, 28, 4, 0, 0)
            .single()
            .expect("yesterday time")
            .timestamp_millis();
        insert_turn(
            &connection,
            "history",
            1,
            yesterday,
            "時間だけで探せる会話",
            "探せます",
        );
        insert_turn(
            &connection,
            "current",
            2,
            now.timestamp_millis(),
            "昨日の会話を見せて",
            "未回答",
        );
        start_run(&connection, "message_2_user");

        let output = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_time_only",
                now,
                timezone,
            },
            RecallConversationInput {
                query: None,
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: None,
            },
        )
        .expect("time-only recall succeeds");

        assert_eq!(output.reason_code, "ok");
        assert_eq!(
            output
                .resolved_time_range
                .as_ref()
                .expect("range resolves")
                .from,
            "2026-08-28T00:00:00+09:00"
        );
        assert!(output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content == "時間だけで探せる会話"));
        assert!(!output
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content == "昨日の会話を見せて"));
    }

    #[test]
    fn cursor_keeps_the_original_time_range_across_local_midnight() {
        let mut connection = database();
        let timezone = chrono_tz::Asia::Tokyo;
        let before_midnight = Utc
            .with_ymd_and_hms(2026, 8, 29, 14, 30, 0)
            .single()
            .expect("fixture time");
        let historical = Utc
            .with_ymd_and_hms(2026, 8, 28, 3, 0, 0)
            .single()
            .expect("historical time")
            .timestamp_millis();
        for sequence in 0..5 {
            insert_turn(
                &connection,
                &format!("history_{sequence}"),
                sequence,
                historical + i64::try_from(sequence).expect("sequence"),
                &format!("pagination-key {sequence}"),
                "neighbor",
            );
        }
        insert_turn(
            &connection,
            "current",
            99,
            before_midnight.timestamp_millis(),
            "過去の続き",
            "未回答",
        );
        start_run(&connection, "message_99_user");
        let filter = RecallTimeFilter::Preset {
            preset: RecallTimePreset::Yesterday,
        };
        let first = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_1",
                now: before_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("pagination-key".to_string()),
                time: Some(filter.clone()),
                cursor: None,
            },
        )
        .expect("first page succeeds");
        assert_eq!(first.windows.len(), 3);
        let cursor = first.next_cursor.clone().expect("next cursor exists");

        insert_turn(
            &connection,
            "late_history",
            77,
            historical + 100,
            "pagination-key inserted after page one",
            "must not enter this cursor snapshot",
        );

        let after_midnight = before_midnight + Duration::hours(2);
        let second = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_2",
                now: after_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("pagination-key".to_string()),
                time: Some(filter),
                cursor: Some(cursor.clone()),
            },
        )
        .expect("second page succeeds");
        assert_eq!(second.windows.len(), 2);
        assert_eq!(second.resolved_time_range, first.resolved_time_range);
        assert!(second.next_cursor.is_none());
        let first_refs = first
            .windows
            .iter()
            .flat_map(|window| window.events.iter().map(|event| event.event_ref.clone()))
            .collect::<HashSet<_>>();
        assert!(second
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .all(|event| !first_refs.contains(&event.event_ref)));
        assert!(!second
            .windows
            .iter()
            .flat_map(|window| &window.events)
            .any(|event| event.content.contains("inserted after page one")));

        let mismatch = execute(
            &mut connection,
            RecallExecutionContext {
                runtime_run_id: "run_recall",
                tool_call_id: "call_page_mismatch",
                now: after_midnight,
                timezone,
            },
            RecallConversationInput {
                query: Some("different-key".to_string()),
                time: Some(RecallTimeFilter::Preset {
                    preset: RecallTimePreset::Yesterday,
                }),
                cursor: Some(cursor),
            },
        )
        .expect_err("cursor cannot be reused with different filters");
        assert_eq!(mismatch.code, RecallErrorCode::CursorFilterMismatch);
    }
}
