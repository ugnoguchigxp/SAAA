//! Host-verified answers for narrow, present-state questions.
//!
//! These cards deliberately cover only facts with a local source of record.  They are not a
//! replacement for normal conversation: an unobservable meeting state, for example, stays
//! `unknown` instead of being inferred from a model completion.

use super::scope::ScopeSnapshot;
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StateAnswer {
    pub(crate) claim: String,
    pub(crate) source_ref: String,
    pub(crate) as_of: String,
    pub(crate) status: String,
}

impl StateAnswer {
    pub(crate) fn render(&self) -> String {
        format!(
            "{}\n\n状態: {}\n確認時刻: {}\n根拠: {}",
            self.claim, self.status, self.as_of, self.source_ref
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Query {
    CurrentTask,
    NextDeadline,
    CurrentMeeting,
}

/// Returns `None` for ordinary conversation. State cards are intentionally opt-in to a narrow
/// vocabulary so a request such as “作業を進めて” still reaches the selected provider.
pub(crate) fn answer(
    connection: &Connection,
    content: &str,
    scope: &ScopeSnapshot,
) -> Result<Option<StateAnswer>, String> {
    let Some(query) = classify(content) else {
        return Ok(None);
    };
    let as_of = now_iso();
    let answer = match query {
        Query::CurrentTask => current_task(connection, scope, as_of),
        Query::NextDeadline => next_deadline(connection, scope, as_of),
        // Situation is an in-memory observation, not a calendar fact. Its authoritative snapshot
        // is injected by the normal context path; this no-provider card must not pretend that a
        // foreground signal proves a meeting.
        Query::CurrentMeeting => StateAnswer {
            claim: "現在の会議状態は、この時点で検証できる観測がありません。".into(),
            source_ref: "situation:unavailable".into(),
            as_of,
            status: "unknown".into(),
        },
    };
    Ok(Some(answer))
}

fn classify(content: &str) -> Option<Query> {
    let value = content.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    if (value.contains("会議") || value.contains("ミーティング") || value.contains("meeting"))
        && (value.contains("今") || value.contains("現在") || value.contains("current"))
    {
        return Some(Query::CurrentMeeting);
    }
    if (value.contains("期限") || value.contains("締切") || value.contains("deadline"))
        && (value.contains("次")
            || value.contains("今")
            || value.contains("current")
            || value.contains("next"))
    {
        return Some(Query::NextDeadline);
    }
    if (value.contains("タスク")
        || value.contains("task")
        || value.contains("作業")
        || value.contains("進め"))
        && (value.contains("今")
            || value.contains("現在")
            || value.contains("何")
            || value.contains("current")
            || value.contains("what"))
    {
        return Some(Query::CurrentTask);
    }
    None
}

fn current_task(connection: &Connection, scope: &ScopeSnapshot, as_of: String) -> StateAnswer {
    let task_id = scope
        .scopes
        .iter()
        .find(|scope| {
            scope.kind == "task" && matches!(scope.relation.as_str(), "current" | "focus")
        })
        .and_then(|scope| scope.key.strip_prefix("task:"));
    let project_id = scope
        .focus_scope_key
        .as_deref()
        .and_then(|key| key.strip_prefix("project:"));
    let row: Result<Option<(String, String, i64)>, String> = match task_id {
        Some(id) => connection
            .query_row(
                "SELECT id,state,revision FROM coding_jobs WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(database_error),
        None => match project_id {
            Some(id) => connection
                .query_row(
                    "SELECT id,state,revision FROM coding_jobs WHERE workspace_id=?1
                     ORDER BY rowid DESC LIMIT 1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(database_error),
            None => Ok(None),
        },
    };
    match row.unwrap_or(None) {
        Some((id, state, revision)) => StateAnswer {
            claim: format!("現在のタスクは {id} で、状態は {state} です。"),
            source_ref: format!("coding_jobs:{id}@{revision}"),
            as_of,
            status: state,
        },
        None => StateAnswer {
            claim: "現在のタスクは、この Scope では確認できません。".into(),
            source_ref: "coding_jobs:unavailable".into(),
            as_of,
            status: "unknown".into(),
        },
    }
}

fn next_deadline(connection: &Connection, scope: &ScopeSnapshot, as_of: String) -> StateAnswer {
    let keys = scope
        .scopes
        .iter()
        .map(|scope| scope.key.as_str())
        .collect::<Vec<_>>();
    let row = keys.into_iter().find_map(|scope_key| {
        connection
            .query_row(
                "SELECT id,due_at,revision,status FROM schedule_entries
                 WHERE scope_ref=?1 AND status IN ('scheduled','firing') ORDER BY due_at LIMIT 1",
                params![scope_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()
    });
    match row {
        Some((id, due_at, revision, status)) => StateAnswer {
            claim: format!("次の期限は {id} で、予定時刻は {due_at} です。"),
            source_ref: format!("schedule_entries:{id}@{revision}"),
            as_of,
            status,
        },
        None => StateAnswer {
            claim: "次の期限は、この Scope では確認できません。".into(),
            source_ref: "schedule_entries:unavailable".into(),
            as_of,
            status: "unknown".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(task: Option<&str>) -> ScopeSnapshot {
        ScopeSnapshot {
            status: "resolved".into(),
            focus_scope_key: Some("project:workspace-1".into()),
            digest: "scope".into(),
            reason_code: None,
            scopes: task
                .into_iter()
                .map(|id| super::super::scope::ResolvedScope {
                    key: format!("task:{id}"),
                    kind: "task".into(),
                    relation: "current".into(),
                    epoch: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn wd_11_current_task_uses_the_coding_ledger_and_records_as_of() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE coding_jobs(id TEXT, state TEXT, revision INTEGER, workspace_id TEXT);
            INSERT INTO coding_jobs VALUES('job-1','running',4,'workspace-1');").unwrap();
        let answer = answer(&connection, "今何を進めている？", &scope(Some("job-1")))
            .unwrap()
            .unwrap();
        assert_eq!(answer.status, "running");
        assert_eq!(answer.source_ref, "coding_jobs:job-1@4");
        assert!(!answer.as_of.is_empty());
    }

    #[test]
    fn wd_11_unobservable_meeting_is_unknown_not_an_inference() {
        let answer = answer(
            &Connection::open_in_memory().unwrap(),
            "現在会議中？",
            &scope(None),
        )
        .unwrap()
        .unwrap();
        assert_eq!(answer.status, "unknown");
        assert_eq!(answer.source_ref, "situation:unavailable");
    }
}
