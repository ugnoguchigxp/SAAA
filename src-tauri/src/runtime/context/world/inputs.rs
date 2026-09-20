//! Typed, bounded source-of-record snapshot for World-aware adapters.
//!
//! This is intentionally metadata only: paths, prompts, tool output and calendar payload bodies
//! stay with their owners. An unavailable source is represented explicitly instead of being
//! silently omitted and later mistaken for an empty state.

use crate::runtime::context::scope::ScopeSnapshot;
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const MAX_TASKS: usize = 8;
const MAX_DEADLINES: usize = 8;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldSourceSnapshot {
    pub(crate) schema: &'static str,
    pub(crate) scope_digest: String,
    pub(crate) observed_at: String,
    pub(crate) tasks: Vec<TaskSource>,
    pub(crate) deadlines: Vec<DeadlineSource>,
    pub(crate) situation: SourceAvailability,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskSource {
    pub(crate) id: String,
    pub(crate) revision: u64,
    pub(crate) status: String,
    pub(crate) source_ref: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeadlineSource {
    pub(crate) id: String,
    pub(crate) revision: u64,
    pub(crate) status: String,
    pub(crate) due_at: i64,
    pub(crate) source_ref: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceAvailability {
    Unavailable,
}

pub(crate) fn read(
    connection: &Connection,
    scope: &ScopeSnapshot,
) -> Result<WorldSourceSnapshot, String> {
    let task_ids = scope
        .scopes
        .iter()
        .filter(|scope| scope.kind == "task")
        .filter_map(|scope| scope.key.strip_prefix("task:"))
        .take(MAX_TASKS)
        .collect::<Vec<_>>();
    let mut tasks = Vec::with_capacity(task_ids.len());
    for id in task_ids {
        let task = connection
            .query_row(
                "SELECT id,revision,state FROM coding_jobs WHERE id=?1",
                [id],
                |row| {
                    Ok(TaskSource {
                        id: row.get(0)?,
                        revision: row.get(1)?,
                        status: row.get(2)?,
                        source_ref: String::new(),
                    })
                },
            )
            .optional()
            .map_err(database_error)?;
        if let Some(mut task) = task {
            task.source_ref = format!("coding_jobs:{}@{}", task.id, task.revision);
            tasks.push(task);
        }
    }
    let scope_keys = scope
        .scopes
        .iter()
        .map(|scope| scope.key.as_str())
        .collect::<Vec<_>>();
    let mut deadlines = Vec::new();
    for key in scope_keys {
        if deadlines.len() == MAX_DEADLINES {
            break;
        }
        let remaining = (MAX_DEADLINES - deadlines.len()) as i64;
        let mut statement = connection
            .prepare(
                "SELECT id,revision,status,due_at FROM schedule_entries
                 WHERE scope_ref=?1 AND status IN ('scheduled','firing')
                 ORDER BY due_at LIMIT ?2",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map(params![key, remaining], |row| {
                let id: String = row.get(0)?;
                let revision: u64 = row.get(1)?;
                Ok(DeadlineSource {
                    source_ref: format!("schedule_entries:{id}@{revision}"),
                    id,
                    revision,
                    status: row.get(2)?,
                    due_at: row.get(3)?,
                })
            })
            .map_err(database_error)?;
        deadlines.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?,
        );
    }
    deadlines.sort_by_key(|deadline| deadline.due_at);
    deadlines.truncate(MAX_DEADLINES);
    Ok(WorldSourceSnapshot {
        schema: "saaa.world-source-snapshot.v1",
        scope_digest: scope.digest.clone(),
        observed_at: now_iso(),
        tasks,
        deadlines,
        // Situation lives in the runtime monitor and must be sampled there. This SQLite snapshot
        // never fabricates a meeting or attention state from calendar rows.
        situation: SourceAvailability::Unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::context::scope::ResolvedScope;

    #[test]
    fn wd_02_snapshot_uses_source_revisions_and_never_payload_text() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE coding_jobs(id TEXT, revision INTEGER, state TEXT);
            CREATE TABLE schedule_entries(id TEXT, revision INTEGER, status TEXT, due_at INTEGER, scope_ref TEXT);
            INSERT INTO coding_jobs VALUES('job-1',3,'running');
            INSERT INTO schedule_entries VALUES('due-1',2,'scheduled',42,'project:p-1');")
            .unwrap();
        let scope = ScopeSnapshot {
            status: "resolved".into(),
            focus_scope_key: Some("project:p-1".into()),
            digest: "digest".into(),
            reason_code: None,
            scopes: vec![
                ResolvedScope {
                    key: "project:p-1".into(),
                    kind: "project".into(),
                    relation: "focus".into(),
                    epoch: 1,
                },
                ResolvedScope {
                    key: "task:job-1".into(),
                    kind: "task".into(),
                    relation: "current".into(),
                    epoch: 1,
                },
            ],
        };
        let snapshot = read(&connection, &scope).unwrap();
        assert_eq!(snapshot.tasks[0].source_ref, "coding_jobs:job-1@3");
        assert_eq!(snapshot.deadlines[0].source_ref, "schedule_entries:due-1@2");
        assert_eq!(snapshot.situation, SourceAvailability::Unavailable);
    }
}
