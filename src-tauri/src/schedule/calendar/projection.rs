use super::client::{CalError, Client};
use super::{client, encode};
use crate::database_error;
use crate::schedule::ledger::{Entry, Status};
use crate::schedule::{ledger, runtime};
use crate::AppState;
use rusqlite::{params, Connection};
use serde_json::Value;

const MAX_CALLS: usize = 8;

pub(crate) fn enqueue_pending(
    connection: &Connection,
    calendar_id: &str,
    now: i64,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR IGNORE INTO calendar_projections(entry_id, calendar_id, projected_rev, projected_hash, state, next_attempt_at)
             SELECT id, ?1, revision, '', 'pending', ?2 FROM schedule_entries
             WHERE status IN ('scheduled','fired','withdrawn')",
            params![calendar_id, now],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn mark_stale(connection: &Connection, entry_id: &str, now: i64) -> Result<(), String> {
    connection
        .execute(
            "UPDATE calendar_projections SET state='stale', next_attempt_at=?2 WHERE entry_id=?1",
            params![entry_id, now],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn flush(state: &AppState, now: i64) -> Result<usize, String> {
    let settings = state.sqlite_writer.read_serialized(runtime::load)?;
    let Some(calendar_id) = settings.calendar_id.clone() else {
        return Ok(0);
    };
    if !settings.calendar_enabled {
        return Ok(0);
    }
    let Some(token) = super::oauth::ensure_access(state) else {
        return Ok(0);
    };
    let base = state.schedule.http_base();
    let jobs = state
        .sqlite_writer
        .read_serialized(|connection| due_jobs(connection, now))?;
    let mut calls = 0;
    for job in jobs {
        if calls >= MAX_CALLS {
            break;
        }
        match project_one(state, &base, &token, &calendar_id, &job, now) {
            Ok(used) => calls += used,
            Err(CalError::Unreachable) | Err(CalError::Auth) => {
                let _ = notice_calendar(state, now, "unreachable");
                break;
            }
            Err(CalError::Gone) | Err(CalError::NotFound) => {
                mark_calendar_gone(state, now)?;
                break;
            }
            Err(_) => calls += 1,
        }
    }
    Ok(calls)
}

struct Job {
    entry: Entry,
    event_id: Option<String>,
    etag: Option<String>,
    hash: String,
}

fn due_jobs(connection: &Connection, now: i64) -> Result<Vec<Job>, String> {
    let mut statement = connection
        .prepare(
            "SELECT e.id, p.event_id, p.remote_etag, p.projected_hash
             FROM calendar_projections p
             JOIN schedule_entries e ON e.id=p.entry_id
             WHERE p.state IN ('pending','stale') AND (p.next_attempt_at IS NULL OR p.next_attempt_at<=?1)
             LIMIT 8",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(database_error)?;
    let mut jobs = Vec::new();
    for row in rows {
        let (id, event_id, etag, hash) = row.map_err(database_error)?;
        let Some(entry) = ledger::get(connection, &id)? else {
            continue;
        };
        jobs.push(Job {
            entry,
            event_id,
            etag,
            hash,
        });
    }
    Ok(jobs)
}

fn project_one(
    state: &AppState,
    base: &str,
    token: &str,
    calendar_id: &str,
    job: &Job,
    now: i64,
) -> Result<usize, CalError> {
    let classification = state
        .sqlite_writer
        .read_serialized(|connection| {
            runtime::classification_for(
                connection,
                &job.entry.subject_ref,
                job.entry.payload_id.as_deref(),
            )
        })
        .unwrap_or_else(|_| "internal".into());
    let body = payload_body(state, job.entry.payload_id.as_deref());
    let title = encode::event_title(&job.entry, body.as_deref(), &classification);
    let hash = encode::event_hash(&job.entry, &title);
    if hash == job.hash && job.event_id.is_some() {
        remember(
            state,
            &job.entry.id,
            job.event_id.as_deref(),
            &hash,
            job.etag.as_deref(),
            "synced",
            now,
        );
        return Ok(0);
    }
    let body_json = encode::event_body(&job.entry, &title, &hash);
    let client = Client {
        base_url: base.to_string(),
        token: token.to_string(),
    };
    state.schedule.record_api("project");
    if job.entry.status == Status::Superseded {
        if let Some(event_id) = job.event_id.clone() {
            let client = client.clone();
            let calendar_id = calendar_id.to_string();
            let etag = job.etag.clone();
            http_block(async move {
                client
                    .delete(&calendar_id, &event_id, etag.as_deref())
                    .await
            })?;
            remember(state, &job.entry.id, None, &hash, None, "synced", now);
        }
        return Ok(1);
    }
    let remote = if let Some(event_id) = job.event_id.clone() {
        let patch_client = client.clone();
        let patch_calendar = calendar_id.to_string();
        let etag = job.etag.clone().unwrap_or_default();
        let patch_body = body_json.clone();
        match http_block(async move {
            patch_client
                .patch(&patch_calendar, &event_id, &etag, &patch_body)
                .await
        }) {
            Ok(remote) => remote,
            Err(CalError::NotFound) | Err(CalError::Precondition) => {
                recover_or_insert(&client, calendar_id, &job.entry.id, &body_json)?
            }
            Err(error) => return Err(error),
        }
    } else {
        recover_or_insert(&client, calendar_id, &job.entry.id, &body_json)?
    };
    remember(
        state,
        &job.entry.id,
        Some(&remote.id),
        &hash,
        Some(&remote.etag),
        "synced",
        now,
    );
    Ok(1)
}

fn recover_or_insert(
    client: &Client,
    calendar_id: &str,
    entry_id: &str,
    body: &Value,
) -> Result<client::RemoteEvent, CalError> {
    let client = client.clone();
    let calendar_id = calendar_id.to_string();
    let entry_id = entry_id.to_string();
    let body = body.clone();
    let found = {
        let client = client.clone();
        let calendar_id = calendar_id.clone();
        let entry_id = entry_id.clone();
        http_block(async move { client.find_by_entry(&calendar_id, &entry_id).await })
    };
    if let Ok(Some(existing)) = found {
        return Ok(existing);
    }
    http_block(async move { client.insert(&calendar_id, &body).await })
}

fn remember(
    state: &AppState,
    entry_id: &str,
    event_id: Option<&str>,
    hash: &str,
    etag: Option<&str>,
    proj_state: &str,
    now: i64,
) {
    let _ = state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "UPDATE calendar_projections
                 SET event_id=?2, projected_hash=?3, remote_etag=?4,
                     state=?5, remote_updated=?6, attempts=0
                 WHERE entry_id=?1",
                params![entry_id, event_id, hash, etag, proj_state, now],
            )
            .map_err(database_error)?;
        Ok(())
    });
}

fn payload_body(state: &AppState, payload_id: Option<&str>) -> Option<String> {
    let payload_id = payload_id?;
    state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT body FROM schedule_payloads WHERE id=?1",
                    [payload_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(database_error)
        })
        .ok()
        .flatten()
}

fn notice_calendar(state: &AppState, now: i64, code: &str) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        if runtime::notice_once(connection, &format!("calendar:{code}"), now)? {
            crate::schedule::notify::assistant(
                connection,
                &crate::schedule::notify::calendar_error(code),
                now,
            )?;
        }
        runtime::set_error(connection, Some(code))?;
        Ok(())
    })
}

fn mark_calendar_gone(state: &AppState, now: i64) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "UPDATE calendar_projections SET state='remote_deleted', next_attempt_at=?1",
                [now],
            )
            .map_err(database_error)?;
        Ok(())
    })
}

fn http_block<T: Send + 'static>(
    fut: impl std::future::Future<Output = Result<T, CalError>> + Send + 'static,
) -> Result<T, CalError> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut)
    })
    .join()
    .map_err(|_| CalError::Unreachable)?
}
