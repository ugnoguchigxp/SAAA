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
    )?;
    // v2 is additive: old rows retain their user-turn source and remain
    // inspectable.  Do not use a unique conversation index for active goals:
    // the execution slot, not the user's set of background goals, is limited.
    connection.execute_batch(
        "DROP INDEX IF EXISTS steward_one_active_goal;
         CREATE TABLE IF NOT EXISTS steward_event_cursor (
           id INTEGER PRIMARY KEY CHECK(id=1), cursor INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO steward_event_cursor(id,cursor) VALUES(1,0);
         CREATE TABLE IF NOT EXISTS steward_origin_bindings (
           id TEXT PRIMARY KEY,
           goal_id TEXT NOT NULL REFERENCES steward_goals(id),
           origin_kind TEXT NOT NULL CHECK(origin_kind IN ('user_turn','delegated_event')),
           origin_id TEXT NOT NULL,
           operation_digest TEXT NOT NULL,
           created_at TEXT NOT NULL,
           UNIQUE(origin_kind,origin_id,operation_digest)
         );
         CREATE TABLE IF NOT EXISTS steward_budget_reservations (
           id TEXT PRIMARY KEY,
           delegation_id TEXT NOT NULL REFERENCES steward_delegations(id),
           task_id TEXT NOT NULL REFERENCES steward_tasks(id),
           state TEXT NOT NULL CHECK(state IN ('reserved','consumed','released')),
           created_at TEXT NOT NULL,
           UNIQUE(task_id)
         );
         CREATE TABLE IF NOT EXISTS steward_dispatch_intents (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL REFERENCES steward_tasks(id),
           state TEXT NOT NULL CHECK(state IN ('pending','dispatching','accepted','failed','outcome_unknown')),
           idempotency_key TEXT NOT NULL UNIQUE,
           receipt_json TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           UNIQUE(task_id)
         );
         CREATE TABLE IF NOT EXISTS steward_task_plans (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL REFERENCES steward_tasks(id),
           revision INTEGER NOT NULL,
           recipe TEXT NOT NULL CHECK(recipe IN ('read','test_run','read_test')),
           request TEXT NOT NULL,
           selection_mode TEXT NOT NULL,
           policy_revision INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           UNIQUE(task_id,revision)
         );
         CREATE TABLE IF NOT EXISTS steward_goal_plans (
           id TEXT PRIMARY KEY,
           goal_id TEXT NOT NULL REFERENCES steward_goals(id),
           revision INTEGER NOT NULL,
           max_replans INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           UNIQUE(goal_id,revision)
         );
         CREATE TABLE IF NOT EXISTS steward_plan_steps (
           plan_id TEXT NOT NULL REFERENCES steward_goal_plans(id),
           step_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL,
           recipe TEXT NOT NULL CHECK(recipe IN ('read','test_run')),
           verifier TEXT NOT NULL,
           depends_on_json TEXT NOT NULL,
           PRIMARY KEY(plan_id,step_id),
           UNIQUE(plan_id,ordinal)
         );
         DROP INDEX IF EXISTS steward_report_delivery_once;
         CREATE INDEX IF NOT EXISTS steward_report_delivery_lookup
           ON steward_reports(conversation_id,digest);",
    )?;
    connection.execute(
        "UPDATE steward_dispatch_intents SET state='outcome_unknown',updated_at=?1 WHERE state='dispatching'",
        [crate::now_iso()],
    )?;
    add_column(
        connection,
        "steward_goals",
        "summary TEXT NOT NULL DEFAULT ''",
    )?;
    add_column(
        connection,
        "steward_goals",
        "revision INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column(
        connection,
        "steward_goals",
        "verifier TEXT NOT NULL DEFAULT 'user_confirmation_required'",
    )?;
    add_column(
        connection,
        "steward_delegations",
        "revision INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column(connection, "steward_delegations", "source_message_id TEXT")?;
    // Existing active delegations predate durable goal plans.  Backfill the
    // same finite host-selected plan without changing their authority.
    connection.execute_batch(
        "INSERT OR IGNORE INTO steward_goal_plans(id,goal_id,revision,max_replans,created_at)
         SELECT 'migration-goal-plan-' || g.id,g.id,1,2,g.created_at
         FROM steward_goals g
         WHERE NOT EXISTS (SELECT 1 FROM steward_goal_plans p WHERE p.goal_id=g.id);
         INSERT OR IGNORE INTO steward_plan_steps(plan_id,step_id,ordinal,recipe,verifier,depends_on_json)
         SELECT p.id,'read',0,'read',g.verifier,'[]'
         FROM steward_goal_plans p JOIN steward_goals g ON g.id=p.goal_id
         JOIN steward_delegations d ON d.goal_id=g.id WHERE d.ops IN ('read','read_test');
         INSERT OR IGNORE INTO steward_plan_steps(plan_id,step_id,ordinal,recipe,verifier,depends_on_json)
         SELECT p.id,'test',CASE WHEN d.ops='read_test' THEN 1 ELSE 0 END,'test_run',g.verifier,
                CASE WHEN d.ops='read_test' THEN '[\"read\"]' ELSE '[]' END
         FROM steward_goal_plans p JOIN steward_goals g ON g.id=p.goal_id
         JOIN steward_delegations d ON d.goal_id=g.id WHERE d.ops IN ('test_run','read_test');",
    )?;
    add_column(
        connection,
        "steward_tasks",
        "revision INTEGER NOT NULL DEFAULT 1",
    )?;
    add_column(
        connection,
        "steward_reports",
        "available_at_ms INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(connection, "steward_reports", "task_id TEXT")?;
    add_column(
        connection,
        "steward_reports",
        "task_revision INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        connection,
        "steward_reports",
        "destination TEXT NOT NULL DEFAULT 'conversation'",
    )?;
    add_column(connection, "steward_reports", "message_id TEXT")?;
    add_column(
        connection,
        "steward_reports",
        "delivery_state TEXT NOT NULL DEFAULT 'pending'",
    )?;
    add_column(
        connection,
        "steward_reports",
        "speak_requested INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        connection,
        "steward_reports",
        "speech_state TEXT NOT NULL DEFAULT 'not_requested'",
    )?;
    add_column(connection, "steward_reports", "speech_run_id TEXT")?;
    // Audio cannot be atomically committed with the database. A process that
    // was already capable of speaking at shutdown is therefore made explicit
    // unknown and is never replayed automatically after restart.
    connection.execute(
        "UPDATE steward_reports SET speech_state='delivery_unknown'
         WHERE speech_state IN ('starting','playback_started')",
        [],
    )?;
    connection.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS steward_report_task_delivery_once
         ON steward_reports(task_id,task_revision,destination)
         WHERE task_id IS NOT NULL;",
    )?;
    Ok(())
}

fn add_column(connection: &Connection, table: &str, definition: &str) -> rusqlite::Result<()> {
    let name = definition.split_whitespace().next().unwrap_or_default();
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|column| column == name);
    if !exists {
        connection.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))?;
    }
    Ok(())
}
