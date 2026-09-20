//! Persistent-event consumer for delegated work.
//!
//! The cursor is durable, so restarting after a coding terminal event cannot
//! lose the report.  Effects are represented as report-outbox rows in the same
//! transaction; delivery itself remains retryable.
use super::repository as repo;
use rusqlite::Connection;

pub(crate) fn consume(connection: &Connection) -> Result<(), String> {
    let cursor: i64 = connection
        .query_row(
            "SELECT cursor FROM steward_event_cursor WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(crate::database_error)?;
    let mut stmt = connection
        .prepare(
            "SELECT sequence,job_id,kind FROM coding_events WHERE sequence>?1 ORDER BY sequence",
        )
        .map_err(crate::database_error)?;
    let events = stmt
        .query_map([cursor], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(crate::database_error)?;
    let events = events
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    drop(stmt);
    for event in events {
        let (sequence, job, kind) = event;
        if matches!(kind.as_str(), "settled" | "failed" | "interrupted") {
            repo::apply_terminal_event(connection, &job, &kind)?;
        }
        connection
            .execute(
                "UPDATE steward_event_cursor SET cursor=?1 WHERE id=1",
                [sequence],
            )
            .map_err(crate::database_error)?;
    }
    Ok(())
}
