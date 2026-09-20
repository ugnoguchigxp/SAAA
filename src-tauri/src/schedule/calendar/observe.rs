use super::client::{CalError, Client, RemoteEvent};
use crate::database_error;
use crate::schedule::{ledger, notify, runtime};
use crate::AppState;
use rusqlite::{params, Connection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffKind {
    Moved,
    Deleted,
    TitleEdited,
    ForeignEvent,
    Unchanged,
}

impl DiffKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::Deleted => "deleted",
            Self::TitleEdited => "title_edited",
            Self::ForeignEvent => "foreign_event",
            Self::Unchanged => "unchanged",
        }
    }
}

pub(crate) fn classify(
    local: Option<&ledger::Entry>,
    remote: &RemoteEvent,
    echo_etag: Option<&str>,
) -> DiffKind {
    if echo_etag.is_some() && echo_etag == Some(remote.etag.as_str()) {
        return DiffKind::Unchanged;
    }
    if remote.entry_id.is_none() {
        return DiffKind::ForeignEvent;
    }
    if remote.status == "cancelled" {
        return DiffKind::Deleted;
    }
    let Some(entry) = local else {
        return DiffKind::ForeignEvent;
    };
    if remote.start.is_some_and(|start| start != entry.due_at) {
        return DiffKind::Moved;
    }
    if remote.summary.as_ref().is_some_and(|summary| {
        summary != &entry.subject_ref && !summary.starts_with('✓') && !summary.starts_with('✕')
    }) {
        return DiffKind::TitleEdited;
    }
    DiffKind::Unchanged
}

pub(crate) fn sync(state: &AppState, now: i64) -> Result<usize, String> {
    let settings = state.sqlite_writer.read_serialized(runtime::load)?;
    let Some(calendar_id) = settings.calendar_id.clone() else {
        return Ok(0);
    };
    if !settings.calendar_enabled {
        return Ok(0);
    }
    let Some(token) = state.schedule.access() else {
        return Ok(0);
    };
    let token_sync = state
        .sqlite_writer
        .read_serialized(|connection| load_token(connection, &calendar_id))?;
    let client = Client {
        base_url: state.schedule.http_base(),
        token,
    };
    state.schedule.record_api("observe");
    let listed = http_list(client, calendar_id.clone(), token_sync.clone());
    let (events, next, _full) = match listed {
        Ok(value) => value,
        Err(CalError::Gone) => {
            let client = Client {
                base_url: state.schedule.http_base(),
                token: state.schedule.access().unwrap_or_default(),
            };
            match http_list(client, calendar_id.clone(), None) {
                Ok((events, next, _)) => (events, next, true),
                Err(CalError::Unreachable | CalError::Auth) => {
                    notice(state, now, "unreachable")?;
                    return Ok(0);
                }
                Err(_) => return Ok(0),
            }
        }
        Err(CalError::Unreachable | CalError::Auth) => {
            notice(state, now, "unreachable")?;
            return Ok(0);
        }
        Err(_) => return Ok(0),
    };
    let mut stored = 0;
    state.sqlite_writer.write(|connection| {
        for remote in &events {
            stored += store_observation(connection, remote, now)?;
        }
        if let Some(next) = next {
            connection
                .execute(
                    "INSERT INTO calendar_sync_state(calendar_id, sync_token, last_full_sync)
                     VALUES(?1,?2,?3)
                     ON CONFLICT(calendar_id) DO UPDATE SET sync_token=?2, last_full_sync=?3",
                    params![calendar_id, next, now],
                )
                .map_err(database_error)?;
        }
        crate::schedule::calendar::reconcile::apply(connection, state, now)?;
        Ok(())
    })?;
    Ok(stored)
}

fn load_token(connection: &Connection, calendar_id: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT sync_token FROM calendar_sync_state WHERE calendar_id=?1",
            [calendar_id],
            |row| row.get(0),
        )
        .optional_mapped()
}

fn observation_open(
    connection: &Connection,
    event_id: &str,
    diff: DiffKind,
) -> Result<bool, String> {
    let sql = match diff {
        DiffKind::Moved | DiffKind::Deleted => {
            "SELECT EXISTS(SELECT 1 FROM calendar_observations WHERE event_id=?1 AND diff_kind=?2 AND handled IN ('pending','asked'))"
        }
        DiffKind::TitleEdited | DiffKind::ForeignEvent => {
            "SELECT EXISTS(SELECT 1 FROM calendar_observations WHERE event_id=?1 AND diff_kind=?2 AND handled IN ('pending','asked','applied'))"
        }
        DiffKind::Unchanged => return Ok(true),
    };
    connection
        .query_row(sql, params![event_id, diff.as_str()], |row| row.get(0))
        .map_err(database_error)
}

trait OptionalMapped<T> {
    fn optional_mapped(self) -> Result<Option<T>, String>;
}

impl OptionalMapped<String> for rusqlite::Result<String> {
    fn optional_mapped(self) -> Result<Option<String>, String> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(database_error(error)),
        }
    }
}

fn store_observation(
    connection: &Connection,
    remote: &RemoteEvent,
    now: i64,
) -> Result<usize, String> {
    let local = match remote.entry_id.as_deref() {
        Some(id) => ledger::get(connection, id)?,
        None => None,
    };
    let echo = connection
        .query_row(
            "SELECT remote_etag FROM calendar_projections WHERE event_id=?1",
            [&remote.id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional_etag()?;
    let diff = classify(local.as_ref(), remote, echo.as_deref());
    if diff == DiffKind::Unchanged || observation_open(connection, &remote.id, diff)? {
        return Ok(0);
    }
    let id = crate::new_id("scho");
    connection
        .execute(
            "INSERT INTO calendar_observations(
               id, event_id, entry_id, observed_at, remote_etag, remote_start, remote_end,
               remote_status, remote_summary, remote_rev, diff_kind, handled
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending')",
            params![
                id,
                remote.id,
                remote.entry_id,
                now,
                remote.etag,
                remote.start,
                remote.end,
                remote.status,
                remote.summary,
                remote.rev,
                diff.as_str()
            ],
        )
        .map_err(database_error)?;
    Ok(1)
}

trait OptionalEtag {
    fn optional_etag(self) -> Result<Option<String>, String>;
}
impl OptionalEtag for rusqlite::Result<Option<String>> {
    fn optional_etag(self) -> Result<Option<String>, String> {
        match self {
            Ok(value) => Ok(value),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(database_error(error)),
        }
    }
}

fn notice(state: &AppState, now: i64, code: &str) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        if runtime::notice_once(connection, &format!("calendar:{code}"), now)? {
            notify::assistant(connection, &notify::calendar_error(code), now)?;
        }
        Ok(())
    })
}

fn http_list(
    client: Client,
    calendar_id: String,
    token: Option<String>,
) -> Result<(Vec<RemoteEvent>, Option<String>, bool), CalError> {
    let full = token.is_none();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async move { client.list(&calendar_id, token.as_deref()).await })
    })
    .join()
    .map_err(|_| CalError::Unreachable)?
    .map(|(events, next)| (events, next, full))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::ledger::{Kind, Origin, Status};

    fn entry(due: i64) -> ledger::Entry {
        ledger::Entry {
            id: "e1".into(),
            kind: Kind::Reminder,
            subject_ref: "goal:g".into(),
            scope_ref: "scope:primary".into(),
            due_at: due,
            window_end_at: None,
            status: Status::Scheduled,
            origin: Origin::UserExplicit,
            delegation_ref: Some("d".into()),
            revision: 2,
            supersedes: None,
            created_at: 1,
            fired_at: None,
            fire_result: None,
            payload_id: None,
        }
    }

    fn remote(entry_id: Option<&str>, start: i64, etag: &str, summary: &str) -> RemoteEvent {
        RemoteEvent {
            id: "ev".into(),
            etag: etag.into(),
            status: "confirmed".into(),
            summary: Some(summary.into()),
            start: Some(start),
            end: None,
            entry_id: entry_id.map(str::to_string),
            rev: Some(1),
            hash: None,
        }
    }

    #[test]
    fn sl_15_classifies_diffs() {
        let local = entry(10);
        assert_eq!(
            classify(Some(&local), &remote(None, 10, "1", "x"), None),
            DiffKind::ForeignEvent
        );
        assert_eq!(
            classify(Some(&local), &remote(Some("e1"), 99, "1", "goal:g"), None),
            DiffKind::Moved
        );
        assert_eq!(
            classify(Some(&local), &remote(Some("e1"), 10, "1", "edited"), None),
            DiffKind::TitleEdited
        );
        assert_eq!(
            classify(
                Some(&local),
                &remote(Some("e1"), 10, "echo", "goal:g"),
                Some("echo")
            ),
            DiffKind::Unchanged
        );
        let mut cancelled = remote(Some("e1"), 10, "1", "goal:g");
        cancelled.status = "cancelled".into();
        assert_eq!(classify(Some(&local), &cancelled, None), DiffKind::Deleted);
    }
}
