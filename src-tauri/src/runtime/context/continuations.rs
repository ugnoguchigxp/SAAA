//! Durable execution continuations that must survive context reduction.
//!
//! These are compact references to host-owned execution state, never copies of tool payloads or
//! grants of authority. The execution boundary still authorizes each operation independently.

use super::source::{Candidate, Requirement};
use crate::database_error;
use rusqlite::{params, Connection};
use serde_json::json;

pub(crate) fn load(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<Candidate>, String> {
    let mut candidates = coding_candidates(connection, conversation_id)?;
    candidates.extend(delegation_candidates(connection, conversation_id)?);
    Ok(candidates)
}

fn coding_candidates(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<Candidate>, String> {
    let mut statement = connection
        .prepare(
            "SELECT j.id,j.revision,j.state,j.current_run_id,r.state,r.delivery
             FROM coding_jobs j JOIN coding_runs r ON r.id=j.current_run_id
             WHERE j.conversation_id=?1
               AND r.state IN ('starting','running','stopping','outcome_unknown')
             ORDER BY j.rowid",
        )
        .map_err(database_error)?;
    let candidates = statement
        .query_map([conversation_id], |row| {
            let id: String = row.get(0)?;
            let revision: u64 = row.get::<_, i64>(1)? as u64;
            let content = json!({
                "kind": "coding-job",
                "id": id,
                "revision": revision,
                "state": row.get::<_, String>(2)?,
                "runId": row.get::<_, String>(3)?,
                "runState": row.get::<_, String>(4)?,
                "delivery": row.get::<_, String>(5)?,
            })
            .to_string();
            Ok(Candidate::untrusted(
                format!("task-continuation:{id}:{revision}"),
                "task-continuation",
                Vec::new(),
                Requirement::Must,
                id,
                revision,
                u16::MAX,
                content,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(candidates)
}

fn delegation_candidates(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<Candidate>, String> {
    let mut statement = connection
        .prepare(
            "SELECT t.id,t.revision,t.loop_state,d.id,d.revision,d.ops,g.id,g.revision,g.summary
             FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.conversation_id=?1
               AND t.loop_state IN ('queued','running','awaiting_user')
               AND d.status='active' AND d.superseded_by IS NULL
               AND g.status='active' AND g.superseded_by IS NULL
             ORDER BY t.rowid",
        )
        .map_err(database_error)?;
    let candidates = statement
        .query_map(params![conversation_id], |row| {
            let task_id: String = row.get(0)?;
            let task_revision: u64 = row.get::<_, i64>(1)? as u64;
            let content = json!({
                "kind": "delegated-task",
                "taskId": task_id,
                "taskRevision": task_revision,
                "taskState": row.get::<_, String>(2)?,
                "delegationId": row.get::<_, String>(3)?,
                "delegationRevision": row.get::<_, i64>(4)?,
                "operations": row.get::<_, String>(5)?,
                "goalId": row.get::<_, String>(6)?,
                "goalRevision": row.get::<_, i64>(7)?,
                "summary": row.get::<_, String>(8)?,
            })
            .to_string();
            Ok(Candidate::untrusted(
                format!("delegation-continuation:{task_id}:{task_revision}"),
                "delegation-continuation",
                Vec::new(),
                Requirement::Must,
                task_id,
                task_revision,
                u16::MAX,
                content,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_only_live_execution_references() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE coding_jobs(id TEXT, conversation_id TEXT, revision INTEGER, state TEXT, current_run_id TEXT);
             CREATE TABLE coding_runs(id TEXT, state TEXT, delivery TEXT);
             CREATE TABLE steward_goals(id TEXT, status TEXT, superseded_by TEXT, revision INTEGER, summary TEXT);
             CREATE TABLE steward_delegations(id TEXT, goal_id TEXT, status TEXT, superseded_by TEXT, revision INTEGER, ops TEXT);
             CREATE TABLE steward_tasks(id TEXT, delegation_id TEXT, conversation_id TEXT, revision INTEGER, loop_state TEXT);",
        ).unwrap();
        connection.execute_batch(
            "INSERT INTO coding_runs VALUES('run-live','running','accepted'),('run-done','settled','accepted');
             INSERT INTO coding_jobs VALUES('job-live','c',3,'running','run-live'),('job-done','c',4,'settled','run-done');
             INSERT INTO steward_goals VALUES('goal-live','active',NULL,2,'read docs'),('goal-old','withdrawn',NULL,2,'old');
             INSERT INTO steward_delegations VALUES('delegation-live','goal-live','active',NULL,4,'read_test'),('delegation-old','goal-old','withdrawn',NULL,2,'read');
             INSERT INTO steward_tasks VALUES('task-live','delegation-live','c',5,'running'),('task-old','delegation-old','c',2,'cancelled');",
        ).unwrap();
        let candidates = load(&connection, "c").unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates
            .iter()
            .all(|candidate| candidate.requirement == Requirement::Must));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.source_kind == "task-continuation"));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.source_kind == "delegation-continuation"));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.content.contains("job-done")));
        assert!(!candidates
            .iter()
            .any(|candidate| candidate.content.contains("task-old")));
    }
}
