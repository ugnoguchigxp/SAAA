//! Persistence of the source ledger (`worker_sources`, `worker_url_blocklist`) and the terminal
//! audit attributes (§6.6). Page bodies are never stored; blocklist rows are never deleted here.
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::redact::redact_excerpt;
use super::sources::{SourceBook, SourceEntry, SourceKind, SourceStatus};
use crate::persistence::SqliteWriter;
use crate::worker_agents::loader::now_ms;

pub(crate) const MAX_AUDIT_ATTRIBUTES_BYTES: usize = 2048;

/// One `worker_sources` row to write.
#[derive(Debug, Clone)]
pub(crate) struct SourceRecord<'a> {
    pub url: &'a str,
    pub host: &'a str,
    pub kind: SourceKind,
    pub status: SourceStatus,
    pub guard_decision: Option<&'a str>,
    pub warning_categories: &'a [String],
    pub check_suspected: Option<bool>,
    pub check_categories: &'a [String],
    /// Raw checker excerpt; redacted and capped here before it reaches the database.
    pub check_excerpt: Option<&'a str>,
    pub fail_reason: Option<&'a str>,
}

impl<'a> SourceRecord<'a> {
    pub(crate) fn new(url: &'a str, host: &'a str, kind: SourceKind, status: SourceStatus) -> Self {
        Self {
            url,
            host,
            kind,
            status,
            guard_decision: None,
            warning_categories: &[],
            check_suspected: None,
            check_categories: &[],
            check_excerpt: None,
            fail_reason: None,
        }
    }
}

pub(crate) fn upsert_source(
    writer: &SqliteWriter,
    task_id: &str,
    record: &SourceRecord<'_>,
) -> Result<(), String> {
    let excerpt = record
        .check_excerpt
        .map(redact_excerpt)
        .filter(|value| !value.is_empty());
    let warnings = serde_json::to_string(record.warning_categories).map_err(|e| e.to_string())?;
    let categories = serde_json::to_string(record.check_categories).map_err(|e| e.to_string())?;
    writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO worker_sources(task_id,url,host,kind,status,guard_decision,warning_categories_json,
                    check_suspected,check_categories_json,check_excerpt,fail_reason,created_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                 ON CONFLICT(task_id,kind,url) DO UPDATE SET
                    status=excluded.status, guard_decision=excluded.guard_decision,
                    warning_categories_json=excluded.warning_categories_json,
                    check_suspected=excluded.check_suspected,
                    check_categories_json=excluded.check_categories_json,
                    check_excerpt=excluded.check_excerpt, fail_reason=excluded.fail_reason",
                params![
                    task_id,
                    record.url,
                    record.host,
                    record.kind.as_str(),
                    record.status.as_str(),
                    record.guard_decision,
                    warnings,
                    record.check_suspected,
                    categories,
                    excerpt,
                    record.fail_reason,
                    now_ms(),
                ],
            )
            .map(|_| ())
            .map_err(crate::database_error)
    })
}

/// A failed source stays failed for every kind it was recorded under (hit, fetched, supplied).
pub(crate) fn mark_url_failed(
    writer: &SqliteWriter,
    task_id: &str,
    url: &str,
    reason: &str,
) -> Result<(), String> {
    writer.write(|connection| {
        connection
            .execute(
                "UPDATE worker_sources SET status='failed', fail_reason=COALESCE(fail_reason, ?3)
                 WHERE task_id=?1 AND url=?2 AND status<>'failed'",
                params![task_id, url, reason],
            )
            .map(|_| ())
            .map_err(crate::database_error)
    })
}

/// Permanent: `INSERT OR IGNORE`, no expiry. Only the user's IPC removal ever deletes rows.
pub(crate) fn insert_blocklist(
    writer: &SqliteWriter,
    url_hash: &str,
    host: &str,
    reason: &str,
) -> Result<(), String> {
    writer.write(|connection| {
        connection
            .execute(
                "INSERT OR IGNORE INTO worker_url_blocklist(url_hash,host,reason,created_at_ms)
                 VALUES(?1,?2,?3,?4)",
                params![url_hash, host, reason, now_ms()],
            )
            .map(|_| ())
            .map_err(crate::database_error)
    })
}

pub(crate) fn is_blocklisted(writer: &SqliteWriter, url_hash: &str) -> Result<bool, String> {
    writer.write(|connection| {
        connection
            .query_row(
                "SELECT 1 FROM worker_url_blocklist WHERE url_hash=?1",
                params![url_hash],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(crate::database_error)
    })
}

/// Reloads the sources recorded by earlier attempts of this task so a new attempt keeps failed
/// sources failed and host exclusions in force.
pub(crate) fn load_sources(writer: &SqliteWriter, task_id: &str) -> Result<SourceBook, String> {
    let rows: Vec<(String, String, String, String, String, String)> =
        writer.write(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT url,host,kind,status,warning_categories_json,check_categories_json
                 FROM worker_sources WHERE task_id=?1",
                )
                .map_err(crate::database_error)?;
            let rows = statement
                .query_map(params![task_id], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .map_err(crate::database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::database_error)?;
            Ok(rows)
        })?;
    let mut book = SourceBook::default();
    for (url, host, kind, status, warnings, checks) in rows {
        let (Some(kind), Some(status)) = (SourceKind::parse(&kind), SourceStatus::parse(&status))
        else {
            continue;
        };
        if kind == SourceKind::SearchHit {
            book.hits_seen += 1;
        }
        if matches!(status, SourceStatus::Failed | SourceStatus::Excluded) {
            for json in [warnings, checks] {
                let names: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
                book.add_categories(names.iter().map(String::as_str));
            }
        }
        book.insert(
            kind,
            &url,
            SourceEntry {
                status,
                host,
                date: None,
            },
        );
    }
    book.rebuild_host_flags();
    Ok(book)
}

/// Counts for the terminal audit row, read from the ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskStats {
    pub usable: u32,
    pub failed: u32,
    pub excluded: u32,
    pub categories: Vec<String>,
    pub searches: u32,
    pub fetches: u32,
}

pub(crate) fn task_stats(writer: &SqliteWriter, task_id: &str) -> Result<TaskStats, String> {
    let book = load_sources(writer, task_id)?;
    let (searches, fetches) = writer.write(|connection| {
        let count = |tool: &str| -> Result<u32, String> {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM worker_tool_calls WHERE task_id=?1 AND tool_key=?2",
                    params![task_id, tool],
                    |row| row.get::<_, u32>(0),
                )
                .map_err(crate::database_error)
        };
        Ok((count("web_search")?, count("fetch_content")?))
    })?;
    Ok(TaskStats {
        usable: book.usable_count(),
        failed: book.failed_count(),
        excluded: book.excluded_count(),
        categories: book.categories(),
        searches,
        fetches,
    })
}

/// Facts the executor knows at task termination.
#[derive(Debug, Clone)]
pub(crate) struct TerminalAuditInput<'a> {
    pub profile_id: &'a str,
    pub revision: u32,
    pub tier: &'a str,
    pub attempts: u32,
    pub steps: u32,
    pub failure_code: Option<&'a str>,
    pub elapsed_ms: u64,
    pub stats: &'a TaskStats,
}

/// Attributes of the `worker` / `worker-task-terminal` audit event, serialized to at most 2048
/// bytes (categories are trimmed first, then dropped, if the limit would be exceeded).
pub(crate) fn terminal_audit_attributes(input: &TerminalAuditInput<'_>) -> Value {
    let mut categories: Vec<&str> = input
        .stats
        .categories
        .iter()
        .map(String::as_str)
        .take(16)
        .collect();
    loop {
        let value = json!({
            "profile": input.profile_id.chars().take(40).collect::<String>(),
            "revision": input.revision,
            "tier": input.tier.chars().take(16).collect::<String>(),
            "attempts": input.attempts,
            "steps": input.steps,
            "searches": input.stats.searches,
            "fetches": input.stats.fetches,
            "usable": input.stats.usable,
            "failed": input.stats.failed,
            "excluded": input.stats.excluded,
            "categories": categories,
            "failureCode": input.failure_code,
            "elapsedMs": input.elapsed_ms,
        });
        if value.to_string().len() <= MAX_AUDIT_ATTRIBUTES_BYTES || categories.is_empty() {
            return value;
        }
        categories.pop();
    }
}
