use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS steward_goals (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           origin TEXT NOT NULL CHECK(origin = 'user_explicit'),
           success_condition TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('active','paused','withdrawn')),
           created_at TEXT NOT NULL,
           superseded_by TEXT
         );
         CREATE TABLE IF NOT EXISTS steward_delegations (
           id TEXT PRIMARY KEY,
           goal_id TEXT NOT NULL REFERENCES steward_goals(id),
           conversation_id TEXT NOT NULL,
           workspace_id TEXT NOT NULL,
           ops TEXT NOT NULL CHECK(ops IN ('read','test_run','read_test')),
           budget_runs INTEGER NOT NULL CHECK(budget_runs > 0),
           budget_ms INTEGER NOT NULL CHECK(budget_ms > 0),
           notify TEXT NOT NULL CHECK(notify IN ('both','silent','speak')),
           status TEXT NOT NULL CHECK(status IN ('active','paused','withdrawn')),
           created_at TEXT NOT NULL,
           superseded_by TEXT
         );
         CREATE TABLE IF NOT EXISTS steward_tasks (
           id TEXT PRIMARY KEY,
           delegation_id TEXT NOT NULL REFERENCES steward_delegations(id),
           conversation_id TEXT NOT NULL,
           trigger_kind TEXT NOT NULL CHECK(trigger_kind IN ('start','continue')),
           source_id TEXT NOT NULL,
           dedupe_key TEXT NOT NULL,
           loop_state TEXT NOT NULL CHECK(loop_state IN ('queued','running','awaiting_user','done','failed','cancelled')),
           coding_job_id TEXT,
           report_json TEXT,
           last_error TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_runtime (
           conversation_id TEXT PRIMARY KEY,
           last_foreground TEXT
         );
         CREATE TABLE IF NOT EXISTS steward_reports (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           digest TEXT NOT NULL,
           held_reason TEXT,
           flushed INTEGER NOT NULL CHECK(flushed IN (0,1)),
           created_at TEXT NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS steward_one_active_goal
           ON steward_goals(conversation_id)
           WHERE status = 'active' AND superseded_by IS NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS steward_active_task_dedupe
           ON steward_tasks(dedupe_key)
           WHERE loop_state IN ('queued','running','awaiting_user');",
    )
}
