use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub const LEDGER_DDL: &str = "
CREATE TABLE IF NOT EXISTS conversation_events (
  conversation_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  run_id TEXT,
  kind TEXT NOT NULL,
  message_id TEXT,
  tool_invocation_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (conversation_id, seq),
  FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS conversation_run_inputs (
  conversation_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  accept_order INTEGER NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending', 'consumed', 'transferred')),
  PRIMARY KEY (conversation_id, run_id, message_id),
  UNIQUE (conversation_id, run_id, accept_order),
  FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
  FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS conversation_run_input_gate (
  run_id TEXT PRIMARY KEY,
  closed INTEGER NOT NULL CHECK(closed IN (0, 1))
);
CREATE TABLE IF NOT EXISTS conversation_message_presentation (
  message_id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending', 'presented', 'unconfirmed')),
  updated_at TEXT NOT NULL,
  FOREIGN KEY(message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS conversation_work_state (
  work_id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  run_id TEXT,
  revision INTEGER NOT NULL,
  purpose TEXT NOT NULL,
  confirmed_refs_json TEXT NOT NULL,
  open_questions_json TEXT NOT NULL,
  next_options_json TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_conversation_work_run
  ON conversation_work_state(run_id) WHERE run_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS conversation_work_runs (
  run_id TEXT PRIMARY KEY,
  work_id TEXT NOT NULL REFERENCES conversation_work_state(work_id) ON DELETE CASCADE
);
INSERT OR IGNORE INTO conversation_work_runs(run_id,work_id)
  SELECT run_id,work_id FROM conversation_work_state WHERE run_id IS NOT NULL;
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkState {
    pub work_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub revision: i64,
    pub purpose: String,
    pub confirmed_refs_json: String,
    pub open_questions_json: String,
    pub next_options_json: String,
    pub status: String,
    pub updated_at: String,
}

pub fn ensure_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(LEDGER_DDL)
}

pub fn commit_visible_message(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    message_id: &str,
    role: &str,
    content: &str,
    created_at: &str,
    run_id: Option<&str>,
    kind: &str,
) -> rusqlite::Result<i64> {
    transaction.execute(
        "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![message_id, conversation_id, role, content, created_at],
    )?;
    append_event(
        transaction,
        conversation_id,
        run_id,
        kind,
        Some(message_id),
        None,
        created_at,
    )
}

pub fn append_event(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: Option<&str>,
    kind: &str,
    message_id: Option<&str>,
    tool_invocation_id: Option<&str>,
    created_at: &str,
) -> rusqlite::Result<i64> {
    let seq: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM conversation_events WHERE conversation_id = ?1",
        params![conversation_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO conversation_events(
           conversation_id, seq, run_id, kind, message_id, tool_invocation_id, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            conversation_id,
            seq,
            run_id,
            kind,
            message_id,
            tool_invocation_id,
            created_at
        ],
    )?;
    Ok(seq)
}

pub fn accept_run_input(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
) -> rusqlite::Result<i64> {
    accept_run_input_with_state(transaction, conversation_id, run_id, message_id, "pending")
}

fn accept_run_input_with_state(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
    state: &str,
) -> rusqlite::Result<i64> {
    let order: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(accept_order), 0) + 1
         FROM conversation_run_inputs WHERE conversation_id = ?1 AND run_id = ?2",
        params![conversation_id, run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO conversation_run_inputs(
           conversation_id, run_id, message_id, accept_order, state
         ) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![conversation_id, run_id, message_id, order, state],
    )?;
    Ok(order)
}

pub fn consume_pending_inputs(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = transaction.prepare(
        "SELECT message_id FROM conversation_run_inputs
         WHERE conversation_id = ?1 AND run_id = ?2 AND state = 'pending'
         ORDER BY accept_order",
    )?;
    let ids = statement
        .query_map(params![conversation_id, run_id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    drop(statement);
    transaction.execute(
        "UPDATE conversation_run_inputs SET state = 'consumed'
         WHERE conversation_id = ?1 AND run_id = ?2 AND state = 'pending'",
        params![conversation_id, run_id],
    )?;
    Ok(ids)
}

pub fn update_work_state(
    transaction: &Transaction<'_>,
    next: &WorkState,
    expected_revision: Option<i64>,
) -> rusqlite::Result<bool> {
    match expected_revision {
        None => {
            let changed = transaction.execute(
                "INSERT INTO conversation_work_state(
                   work_id, conversation_id, run_id, revision, purpose,
                   confirmed_refs_json, open_questions_json, next_options_json, status, updated_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    next.work_id,
                    next.conversation_id,
                    next.run_id,
                    next.revision,
                    next.purpose,
                    next.confirmed_refs_json,
                    next.open_questions_json,
                    next.next_options_json,
                    next.status,
                    next.updated_at
                ],
            )?;
            Ok(changed == 1)
        }
        Some(revision) => {
            let changed = transaction.execute(
                "UPDATE conversation_work_state
                 SET run_id = ?2, revision = ?3, purpose = ?4, confirmed_refs_json = ?5,
                     open_questions_json = ?6, next_options_json = ?7, status = ?8, updated_at = ?9
                 WHERE work_id = ?1 AND revision = ?10",
                params![
                    next.work_id,
                    next.run_id,
                    next.revision,
                    next.purpose,
                    next.confirmed_refs_json,
                    next.open_questions_json,
                    next.next_options_json,
                    next.status,
                    next.updated_at,
                    revision
                ],
            )?;
            Ok(changed == 1)
        }
    }
}

pub fn begin_work(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
    purpose: &str,
    transferred_message_id: Option<&str>,
) -> rusqlite::Result<()> {
    let previous: Option<(String, i64)> = match transferred_message_id {
        Some(message_id) => transaction
            .query_row(
                "SELECT w.work_id,w.revision FROM conversation_run_inputs i
                 JOIN conversation_work_runs history ON history.run_id=i.run_id
                 JOIN conversation_work_state w ON w.work_id=history.work_id
                 WHERE i.conversation_id=?1 AND i.message_id=?2
                 ORDER BY i.accept_order LIMIT 1",
                params![conversation_id, message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?,
        None => None,
    };
    let now = crate::now_iso();
    let work_id = if let Some((work_id, revision)) = previous {
        transaction.execute(
            "UPDATE conversation_work_state SET run_id=?2, revision=?3, purpose=?4,
               status='running', updated_at=?5 WHERE work_id=?1 AND revision=?6",
            params![work_id, run_id, revision + 1, purpose, now, revision],
        )?;
        work_id
    } else {
        let work_id = crate::new_id("work");
        transaction.execute(
            "INSERT INTO conversation_work_state(
               work_id,conversation_id,run_id,revision,purpose,confirmed_refs_json,
               open_questions_json,next_options_json,status,updated_at
             ) VALUES(?1,?2,?3,1,?4,'[]','[]','[]','running',?5)",
            params![work_id, conversation_id, run_id, purpose, now],
        )?;
        work_id
    };
    transaction.execute(
        "INSERT INTO conversation_work_runs(run_id,work_id) VALUES(?1,?2)",
        params![run_id, work_id],
    )?;
    Ok(())
}

pub fn record_work_reference(
    connection: &Connection,
    run_id: &str,
    reference: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE conversation_work_state
         SET confirmed_refs_json=json_insert(confirmed_refs_json,'$[#]',?2),
             revision=revision+1,updated_at=?3
         WHERE run_id=?1 AND status='running'",
        params![run_id, reference, crate::now_iso()],
    )?;
    Ok(())
}

pub fn finish_work(connection: &Connection, run_id: &str, status: &str) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE conversation_work_state SET status=?2,revision=revision+1,updated_at=?3
         WHERE run_id=?1 AND status='running'",
        params![run_id, status, crate::now_iso()],
    )?;
    Ok(())
}

pub fn commit_preface(
    connection: &mut Connection,
    conversation_id: &str,
    run_id: &str,
    text: &str,
) -> rusqlite::Result<Option<crate::ipc_contract::ConversationMessage>> {
    let (visible, _) =
        crate::voice::cloud_tts::speech_directive::project_complete_assistant_content(text);
    let visible = visible.trim();
    if visible.is_empty() {
        return Ok(None);
    }
    ensure_schema(connection)?;
    let transaction = connection.transaction()?;
    let message_id = crate::new_id("message");
    let created_at = crate::now_iso();
    commit_visible_message(
        &transaction,
        conversation_id,
        &message_id,
        "assistant",
        visible,
        &created_at,
        Some(run_id),
        "message_committed",
    )?;
    transaction.execute(
        "INSERT INTO conversation_message_presentation(message_id, conversation_id, run_id, state, updated_at)
         VALUES(?1, ?2, ?3, 'pending', ?4)",
        params![message_id, conversation_id, run_id, created_at],
    )?;
    record_work_reference(&transaction, run_id, &format!("message:{message_id}"))?;
    transaction.commit()?;
    Ok(Some(crate::ipc_contract::ConversationMessage {
        id: message_id,
        conversation_id: conversation_id.to_string(),
        role: "assistant".into(),
        content: visible.to_string(),
        parts: None,
        created_at,
    }))
}

pub fn acknowledge_message_presented(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
) -> rusqlite::Result<bool> {
    let changed = connection.execute(
        "UPDATE conversation_message_presentation
         SET state='presented', updated_at=?4
         WHERE conversation_id=?1 AND run_id=?2 AND message_id=?3
           AND state IN ('pending','unconfirmed')",
        params![conversation_id, run_id, message_id, crate::now_iso()],
    )?;
    Ok(changed == 1)
}

pub fn message_was_presented(connection: &Connection, message_id: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_message_presentation
         WHERE message_id=?1 AND state='presented')",
        [message_id],
        |row| row.get(0),
    )
}

pub fn mark_presentation_unconfirmed(
    connection: &Connection,
    message_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE conversation_message_presentation
         SET state='unconfirmed', updated_at=?2
         WHERE message_id=?1 AND state='pending'",
        params![message_id, crate::now_iso()],
    )?;
    Ok(())
}

pub fn record_tool_event(
    connection: &mut Connection,
    conversation_id: &str,
    run_id: &str,
    tool_invocation_id: &str,
    kind: &str,
) -> rusqlite::Result<i64> {
    let transaction = connection.transaction()?;
    let seq = append_event(
        &transaction,
        conversation_id,
        Some(run_id),
        kind,
        None,
        Some(tool_invocation_id),
        &crate::now_iso(),
    )?;
    if kind == "tool_result" {
        record_work_reference(&transaction, run_id, &format!("event:{seq}"))?;
    }
    transaction.commit()?;
    Ok(seq)
}

pub fn attach_to_running(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
    message_id: &str,
) -> rusqlite::Result<bool> {
    let status: Option<String> = transaction
        .query_row(
            "SELECT status FROM runtime_runs
         WHERE id = ?1 AND conversation_id = ?2 AND route_kind = 'conversation.respond'
           AND (status='running' OR
                (status='completed' AND id=(
                   SELECT id FROM runtime_runs
                   WHERE conversation_id=?2 AND route_kind='conversation.respond'
                   ORDER BY rowid DESC LIMIT 1)))",
            params![run_id, conversation_id],
            |row| row.get(0),
        )
        .optional()?;
    if !matches!(status.as_deref(), Some("running" | "completed")) {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    let gate_closed: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_run_input_gate WHERE run_id=?1 AND closed=1)",
        [run_id],
        |row| row.get(0),
    )?;
    let transferred = gate_closed || status.as_deref() == Some("completed");
    accept_run_input_with_state(
        transaction,
        conversation_id,
        run_id,
        message_id,
        if transferred {
            "transferred"
        } else {
            "pending"
        },
    )?;
    Ok(transferred)
}

pub fn first_unconsumed_terminal_input(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<(String, String, String)>> {
    connection
        .query_row(
            "SELECT m.id,m.content,r.status FROM conversation_run_inputs i
             JOIN runtime_runs r ON r.id=i.run_id AND r.conversation_id=i.conversation_id
             JOIN conversation_messages m ON m.id=i.message_id
             WHERE i.conversation_id=?1 AND i.state IN ('pending','transferred')
               AND r.status IN ('completed','failed','cancelled','interrupted')
             ORDER BY m.created_at,i.accept_order,m.id LIMIT 1",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
}

pub fn peek_pending_user_texts(
    connection: &Connection,
    conversation_id: &str,
    run_id: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT i.message_id, m.content
         FROM conversation_run_inputs i
         JOIN conversation_messages m ON m.id = i.message_id
         WHERE i.conversation_id = ?1 AND i.run_id = ?2 AND i.state = 'pending'
         ORDER BY i.accept_order",
    )?;
    let rows = statement
        .query_map(params![conversation_id, run_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn mark_inputs_consumed(
    connection: &mut Connection,
    conversation_id: &str,
    run_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    mark_inputs_consumed_in_transaction(&transaction, conversation_id, run_id, message_ids)?;
    transaction.commit()
}

fn mark_inputs_consumed_in_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    run_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for message_id in message_ids {
        let changed = transaction.execute(
            "UPDATE conversation_run_inputs SET state = 'consumed'
             WHERE conversation_id = ?1 AND run_id = ?2 AND message_id = ?3 AND state = 'pending'",
            params![conversation_id, run_id, message_id],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
    }
    Ok(())
}

/// Consume the input snapshot used by a generation and atomically close
/// admission only if no newer user message is waiting.
pub fn finish_input_round(
    connection: &mut Connection,
    conversation_id: &str,
    run_id: &str,
    consumed_ids: &[String],
) -> rusqlite::Result<bool> {
    let transaction = connection.transaction()?;
    mark_inputs_consumed_in_transaction(&transaction, conversation_id, run_id, consumed_ids)?;
    let late_input: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation_run_inputs
         WHERE conversation_id = ?1 AND run_id = ?2 AND state = 'pending')",
        params![conversation_id, run_id],
        |row| row.get(0),
    )?;
    if !late_input {
        transaction.execute(
            "INSERT INTO conversation_run_input_gate(run_id, closed) VALUES(?1, 1)
             ON CONFLICT(run_id) DO UPDATE SET closed = 1",
            [run_id],
        )?;
    }
    transaction.commit()?;
    Ok(late_input)
}

pub fn load_work_state(
    connection: &Connection,
    work_id: &str,
) -> rusqlite::Result<Option<WorkState>> {
    connection
        .query_row(
            "SELECT work_id, conversation_id, run_id, revision, purpose, confirmed_refs_json,
                    open_questions_json, next_options_json, status, updated_at
             FROM conversation_work_state WHERE work_id = ?1",
            params![work_id],
            |row| {
                Ok(WorkState {
                    work_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    run_id: row.get(2)?,
                    revision: row.get(3)?,
                    purpose: row.get(4)?,
                    confirmed_refs_json: row.get(5)?,
                    open_questions_json: row.get(6)?,
                    next_options_json: row.get(7)?,
                    status: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .optional()
}
