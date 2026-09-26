//! Compatibility DDL for stored conversation history; no execution logic.
use rusqlite::Connection;

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


pub(crate) fn ensure_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(LEDGER_DDL)
}

pub(crate) fn ensure_lfm_history_schema(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS lfm_voice_utterances (
        utterance_id TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        user_message_id TEXT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
        reply_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        status TEXT NOT NULL CHECK(status IN ('pending','respond','delegate','failed')),
        reasoning_request_id TEXT UNIQUE,
        request_message_id TEXT REFERENCES conversation_messages(id) ON DELETE CASCADE,
        claimed_run_id TEXT,
        failure_code TEXT,
        reply_key TEXT CHECK(reply_key IN ('greeting','acknowledgement','thinking'))
    );",
    )?;
    let has_reasoning_request: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lfm_voice_utterances') WHERE name='reasoning_request_id')",
        [], |row| row.get(0),
    )?;
    if !has_reasoning_request {
        c.execute_batch("ALTER TABLE lfm_voice_utterances ADD COLUMN reasoning_request_id TEXT")?;
    }
    let has_legacy_handoff: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lfm_voice_utterances') WHERE name='handoff_id')",
        [], |row| row.get(0),
    )?;
    if has_legacy_handoff {
        c.execute_batch("UPDATE lfm_voice_utterances SET reasoning_request_id=handoff_id WHERE reasoning_request_id IS NULL AND handoff_id IS NOT NULL")?;
    }
    let has_reply_key: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('lfm_voice_utterances') WHERE name='reply_key')",
        [],
        |row| row.get(0),
    )?;
    if !has_reply_key {
        c.execute_batch("ALTER TABLE lfm_voice_utterances ADD COLUMN reply_key TEXT CHECK(reply_key IN ('greeting','acknowledgement','thinking'))")?;
    }
    c.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_lfm_voice_reasoning_request ON lfm_voice_utterances(reasoning_request_id)")
}
