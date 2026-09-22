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
