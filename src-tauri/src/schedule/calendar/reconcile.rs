use crate::database_error;
use crate::schedule::ledger::{Entry, Origin, Status};
use crate::schedule::{ledger, notify, runtime};
use crate::AppState;
use rusqlite::{params, Connection};

struct Observation {
    id: String,
    event_id: String,
    entry_id: Option<String>,
    remote_start: Option<i64>,
    _remote_summary: Option<String>,
    diff: String,
    remote_rev: Option<i64>,
}

pub(crate) fn apply(connection: &Connection, state: &AppState, now: i64) -> Result<(), String> {
    let mut statement = connection
        .prepare(
            "SELECT id, event_id, entry_id, remote_start, remote_summary, diff_kind, remote_rev
             FROM calendar_observations WHERE handled='pending' ORDER BY observed_at LIMIT 32",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(Observation {
                id: row.get(0)?,
                event_id: row.get(1)?,
                entry_id: row.get(2)?,
                remote_start: row.get(3)?,
                _remote_summary: row.get(4)?,
                diff: row.get(5)?,
                remote_rev: row.get(6)?,
            })
        })
        .map_err(database_error)?;
    let observations = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for observation in observations {
        let handled = reconcile_one(connection, state, &observation, now)?;
        connection
            .execute(
                "UPDATE calendar_observations SET handled=?2 WHERE id=?1",
                params![observation.id, handled],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn reconcile_one(
    connection: &Connection,
    _state: &AppState,
    observation: &Observation,
    now: i64,
) -> Result<&'static str, String> {
    match observation.diff.as_str() {
        "unchanged" => Ok("ignored"),
        "moved" => apply_move(connection, observation, now),
        "deleted" => apply_delete(connection, observation, now),
        "title_edited" => {
            if let Some(entry_id) = observation.entry_id.as_deref() {
                if let Some(entry) = ledger::get(connection, entry_id)? {
                    if observation_stale(&entry, observation) {
                        return conflict(connection, &entry, now);
                    }
                }
            }
            notify::assistant(
                connection,
                &format!(
                    "schedule-extract:{}",
                    observation
                        .entry_id
                        .as_deref()
                        .unwrap_or(&observation.event_id)
                ),
                now,
            )?;
            Ok("asked")
        }
        "foreign_event" => {
            if runtime::notice_once(
                connection,
                &format!("foreign:{}", observation.event_id),
                now,
            )? {
                notify::assistant(connection, &notify::foreign(&observation.event_id), now)?;
                Ok("asked")
            } else {
                Ok("ignored")
            }
        }
        _ => Ok("ignored"),
    }
}

fn apply_move(
    connection: &Connection,
    observation: &Observation,
    now: i64,
) -> Result<&'static str, String> {
    let Some(entry_id) = observation.entry_id.as_deref() else {
        return Ok("ignored");
    };
    let Some(old) = ledger::get(connection, entry_id)? else {
        return Ok("ignored");
    };
    if observation_stale(&old, observation) {
        return conflict(connection, &old, now);
    }
    if matches!(old.status, Status::Fired | Status::Withdrawn) {
        return Ok("ignored");
    }
    let mut next = old.clone();
    next.id = crate::new_id("sched");
    next.due_at = observation.remote_start.unwrap_or(old.due_at);
    next.origin = Origin::UserCalendarEdit;
    next.revision = old.revision + 1;
    next.supersedes = Some(old.id.clone());
    next.status = Status::Scheduled;
    next.created_at = now;
    next.fired_at = None;
    next.fire_result = None;
    ledger::supersede(connection, &old, &next)?;
    crate::schedule::calendar::projection::mark_stale(connection, &old.id, now)?;
    Ok("applied")
}

fn apply_delete(
    connection: &Connection,
    observation: &Observation,
    now: i64,
) -> Result<&'static str, String> {
    let Some(entry_id) = observation.entry_id.as_deref() else {
        return Ok("ignored");
    };
    let Some(old) = ledger::get(connection, entry_id)? else {
        return Ok("ignored");
    };
    if matches!(old.status, Status::Fired | Status::Withdrawn) {
        return Ok("ignored");
    }
    if old.delegation_ref.is_some() {
        notify::assistant(connection, &notify::confirm_delete(&old.subject_ref), now)?;
        return Ok("asked");
    }
    ledger::withdraw(connection, &old.id, old.revision)?;
    crate::schedule::calendar::projection::mark_stale(connection, &old.id, now)?;
    Ok("applied")
}

fn observation_stale(entry: &Entry, observation: &Observation) -> bool {
    observation
        .remote_rev
        .is_some_and(|rev| rev < entry.revision)
}

fn conflict(connection: &Connection, entry: &Entry, now: i64) -> Result<&'static str, String> {
    if runtime::notice_once(connection, &format!("conflict:{}", entry.id), now)? {
        notify::assistant(connection, &notify::conflict(&entry.subject_ref), now)?;
    }
    Ok("conflict")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::ledger::{Kind, Origin, Status};
    use crate::schedule::schema::migrate;
    use rusqlite::Connection;

    #[test]
    fn sl_17_moved_creates_new_revision() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversation_messages(
                   id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT, created_at TEXT
                 );",
            )
            .unwrap();
        migrate(&connection).unwrap();
        let old = Entry {
            id: "old".into(),
            kind: Kind::Reminder,
            subject_ref: "goal:g".into(),
            scope_ref: "scope:primary".into(),
            due_at: 10,
            window_end_at: None,
            status: Status::Scheduled,
            origin: Origin::UserExplicit,
            delegation_ref: None,
            revision: 1,
            supersedes: None,
            created_at: 1,
            fired_at: None,
            fire_result: None,
            payload_id: None,
        };
        ledger::insert(&connection, &old).unwrap();
        apply_move(
            &connection,
            &Observation {
                id: "o".into(),
                event_id: "ev".into(),
                entry_id: Some("old".into()),
                remote_start: Some(50),
                _remote_summary: None,
                diff: "moved".into(),
                remote_rev: Some(1),
            },
            9,
        )
        .unwrap();
        assert_eq!(
            ledger::get(&connection, "old").unwrap().unwrap().status,
            Status::Superseded
        );
    }
}
