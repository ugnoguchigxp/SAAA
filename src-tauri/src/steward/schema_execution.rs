//! Additive execution schema for delegated work. Goal authority status stays on
//! `steward_goals`; work progress lives here so reopen cannot recreate a one-Goal index.
use rusqlite::Connection;
use std::collections::HashSet;

const TASK_STATES: &str = "'queued','dispatching','running','awaiting_dependency','awaiting_user','verifying','done','failed','cancelled','outcome_unknown'";

pub(crate) fn migrate_task_loop_states(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let result = migrate_task_loop_states_inner(connection);
    let restored = connection.execute_batch("PRAGMA foreign_keys = ON;");
    result.and(restored)
}

fn migrate_task_loop_states_inner(connection: &Connection) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='steward_tasks')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='steward_tasks'",
        [],
        |row| row.get(0),
    )?;
    if sql.contains("outcome_unknown") && sql.contains("dispatching") {
        return Ok(());
    }
    let columns = connection
        .prepare("PRAGMA table_info(steward_tasks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let revision = if columns.contains("revision") {
        "COALESCE(revision,1)"
    } else {
        "1"
    };
    let goal_plan_id = if columns.contains("goal_plan_id") {
        "goal_plan_id"
    } else {
        "NULL"
    };
    let plan_step_id = if columns.contains("plan_step_id") {
        "plan_step_id"
    } else {
        "NULL"
    };
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(&format!(
        "CREATE TABLE steward_tasks_v31 (
           id TEXT PRIMARY KEY,
           delegation_id TEXT NOT NULL REFERENCES steward_delegations(id),
           conversation_id TEXT NOT NULL,
           trigger_kind TEXT NOT NULL CHECK(trigger_kind IN ('start','continue','admission')),
           source_id TEXT NOT NULL,
           dedupe_key TEXT NOT NULL,
           loop_state TEXT NOT NULL CHECK(loop_state IN ({TASK_STATES})),
           coding_job_id TEXT,
           report_json TEXT,
           last_error TEXT,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           revision INTEGER NOT NULL DEFAULT 1,
           goal_plan_id TEXT,
           plan_step_id TEXT
         );
         INSERT INTO steward_tasks_v31(
           id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,
           coding_job_id,report_json,last_error,created_at,updated_at,revision,goal_plan_id,plan_step_id)
         SELECT id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,
                CASE loop_state
                  WHEN 'queued' THEN 'queued'
                  WHEN 'running' THEN 'running'
                  WHEN 'awaiting_user' THEN 'awaiting_user'
                  WHEN 'done' THEN 'done'
                  WHEN 'failed' THEN 'failed'
                  WHEN 'cancelled' THEN 'cancelled'
                  ELSE 'outcome_unknown'
                END,
                coding_job_id,report_json,last_error,created_at,updated_at,
                {revision},{goal_plan_id},{plan_step_id}
           FROM steward_tasks;
         DROP TABLE steward_tasks;
         ALTER TABLE steward_tasks_v31 RENAME TO steward_tasks;
         CREATE UNIQUE INDEX IF NOT EXISTS steward_active_task_dedupe
           ON steward_tasks(dedupe_key)
           WHERE loop_state IN ('queued','dispatching','running','awaiting_dependency','awaiting_user','verifying');",
    ))?;
    transaction.commit()
}

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS steward_goal_progress (
           goal_id TEXT PRIMARY KEY REFERENCES steward_goals(id),
           work_status TEXT NOT NULL CHECK(work_status IN ('queued','running','awaiting_user','done','failed','cancelled')),
           revision INTEGER NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_proposals (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           source_message_id TEXT NOT NULL,
           revision INTEGER NOT NULL,
           digest TEXT NOT NULL,
           payload_json TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('pending','confirmed','rejected','expired')),
           confirmation_receipt TEXT,
           created_at TEXT NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS steward_proposal_source_digest
           ON steward_proposals(source_message_id,digest);
         CREATE TABLE IF NOT EXISTS steward_source_bindings (
           id TEXT PRIMARY KEY,
           subject_kind TEXT NOT NULL,
           subject_id TEXT NOT NULL,
           source_kind TEXT NOT NULL CHECK(source_kind IN ('user_message','ui_receipt')),
           source_id TEXT NOT NULL,
           source_version TEXT NOT NULL,
           quote_start INTEGER,
           quote_end INTEGER,
           scope_key TEXT,
           scope_epoch INTEGER,
           goal_revision INTEGER,
           delegation_revision INTEGER,
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_recipes (
           id TEXT PRIMARY KEY,
           revision INTEGER NOT NULL,
           digest TEXT NOT NULL,
           name TEXT NOT NULL,
           target TEXT NOT NULL,
           cwd TEXT NOT NULL,
           argv_json TEXT NOT NULL,
           env_allow_json TEXT NOT NULL,
           output_dir TEXT NOT NULL,
           network INTEGER NOT NULL CHECK(network IN (0,1)),
           timeout_ms INTEGER NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_verifier_outcomes (
           id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL REFERENCES steward_tasks(id),
           verifier TEXT NOT NULL,
           outcome TEXT NOT NULL CHECK(outcome IN ('pass','fail','missing','awaiting_user','unknown')),
           evidence_json TEXT NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_delivery_cursor (
           conversation_id TEXT PRIMARY KEY,
           revision INTEGER NOT NULL,
           last_message_id TEXT,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS steward_execution_evidence (
           run_id TEXT PRIMARY KEY,
           task_id TEXT NOT NULL,
           job_id TEXT NOT NULL,
           schema_version INTEGER NOT NULL,
           recipe_id TEXT,
           recipe_revision INTEGER,
           recipe_digest TEXT,
           target_digest TEXT NOT NULL,
           terminal_kind TEXT NOT NULL,
           exit_code INTEGER,
           result_ref TEXT,
           result_digest TEXT,
           producer TEXT NOT NULL,
           readable INTEGER NOT NULL CHECK(readable IN (0,1)),
           reason_code TEXT NOT NULL,
           payload_json TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )?;
    add_column(
        connection,
        "steward_dispatch_intents",
        "next_eligible_at INTEGER",
    )?;
    add_column(connection, "steward_dispatch_intents", "last_reason TEXT")?;
    add_column(
        connection,
        "steward_dispatch_intents",
        "attempt INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(connection, "steward_delegations", "expires_at TEXT")?;
    add_column(connection, "steward_delegations", "target TEXT")?;
    add_column(connection, "steward_delegations", "recipe_id TEXT")?;
    add_column(connection, "steward_plan_steps", "recipe_id TEXT")?;
    add_column(connection, "steward_plan_steps", "capability TEXT")?;
    add_column(connection, "steward_plan_steps", "verifier_input TEXT")?;
    add_column(connection, "steward_reports", "content_json TEXT")?;
    add_column(
        connection,
        "steward_reports",
        "invalidated INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(connection, "steward_verifier_outcomes", "run_id TEXT")?;
    add_column(
        connection,
        "steward_verifier_outcomes",
        "reason_code TEXT NOT NULL DEFAULT ''",
    )?;
    add_column(connection, "steward_goal_progress", "technical_state TEXT")?;
    add_column(
        connection,
        "steward_goal_progress",
        "verified_success INTEGER NOT NULL DEFAULT 0",
    )?;
    connection.execute_batch(
        "INSERT OR IGNORE INTO steward_goal_progress(goal_id,work_status,revision,updated_at)
         SELECT g.id,
                CASE
                  WHEN g.status='withdrawn' THEN 'cancelled'
                  WHEN EXISTS(SELECT 1 FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id WHERE d.goal_id=g.id AND t.loop_state IN ('queued','dispatching','running','awaiting_dependency','awaiting_user','verifying')) THEN 'running'
                  WHEN EXISTS(SELECT 1 FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id WHERE d.goal_id=g.id) THEN 'queued'
                  ELSE 'queued'
                END,
                g.revision,
                g.created_at
         FROM steward_goals g;",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_rebuild_accepts_the_pre_plan_schema() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE steward_delegations(id TEXT PRIMARY KEY);
                 CREATE TABLE steward_tasks(
                   id TEXT PRIMARY KEY, delegation_id TEXT NOT NULL, conversation_id TEXT NOT NULL,
                   trigger_kind TEXT NOT NULL, source_id TEXT NOT NULL, dedupe_key TEXT NOT NULL,
                   loop_state TEXT NOT NULL CHECK(loop_state IN ('queued','running','awaiting_user','done','failed','cancelled')),
                   coding_job_id TEXT, report_json TEXT, last_error TEXT,
                   created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                 );
                 INSERT INTO steward_delegations VALUES('d');
                 INSERT INTO steward_tasks VALUES('t','d','c','start','s','k','running',NULL,NULL,NULL,'1','1');",
            )
            .unwrap();

        migrate_task_loop_states(&connection).unwrap();

        let migrated: (String, i64, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT loop_state,revision,goal_plan_id,plan_step_id FROM steward_tasks WHERE id='t'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(migrated, ("running".into(), 1, None, None));
    }
}
