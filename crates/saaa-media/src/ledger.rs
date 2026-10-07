//! Durable submission identity. No prompt or API key is stored here.
//! Callers own the connection and commit. These functions do neither.
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

use crate::{database_error, MediaError, MediaKind, MediaResult, ResolvedRoute};

pub fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS purpose_media_operations(
        run_id TEXT PRIMARY KEY,kind TEXT NOT NULL,route_json TEXT NOT NULL,
        state TEXT NOT NULL,remote_id TEXT,result_json TEXT,error_json TEXT,updated_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS purpose_media_artifacts(
        run_id TEXT NOT NULL,artifact_index INTEGER NOT NULL,content BLOB NOT NULL,
        PRIMARY KEY(run_id,artifact_index),FOREIGN KEY(run_id) REFERENCES purpose_media_operations(run_id));",
    )
    .map_err(database_error)
}

pub fn reserve(
    tx: &Transaction<'_>,
    now: &str,
    run: &str,
    kind: &MediaKind,
    route: &ResolvedRoute,
    validate: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<(), String> {
    validate(tx)?;
    let inserted = tx
        .execute(
            "INSERT OR IGNORE INTO purpose_media_operations(run_id,kind,route_json,state,updated_at) VALUES(?1,?2,?3,'reserved',?4)",
            params![
                run,
                serde_json::to_string(kind).map_err(|error| error.to_string())?,
                serde_json::to_string(route).map_err(|error| error.to_string())?,
                now
            ],
        )
        .map_err(database_error)?;
    if inserted != 1 {
        return Err("この生成要求は記録済みです。再送せず、進行状況を確認してください。".into());
    }
    Ok(())
}

pub fn phase(
    db: &Connection,
    now: &str,
    run: &str,
    phase: &str,
    job: Option<&str>,
) -> Result<(), String> {
    // A late progress event must not clear cancel_requested or a terminal state.
    let count = db
        .execute(
            "UPDATE purpose_media_operations SET remote_id=COALESCE(?3,remote_id),updated_at=?4,state=CASE WHEN state='cancel_requested' THEN state ELSE ?2 END WHERE run_id=?1 AND state NOT IN ('accepted','cancelled','failed','unknown')",
            params![run, phase, job, now],
        )
        .map_err(database_error)?;
    if count == 1 {
        return Ok(());
    }
    let exists = db
        .query_row(
            "SELECT 1 FROM purpose_media_operations WHERE run_id=?1",
            [run],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(database_error)?;
    if exists.is_some() {
        Ok(())
    } else {
        Err("生成の記録を更新できません".into())
    }
}

pub enum CancelRecord {
    Accepted,
    AlreadyTerminal,
    Missing,
}

pub fn request_cancel(db: &Connection, now: &str, run: &str) -> Result<CancelRecord, String> {
    let state: Option<String> = db
        .query_row(
            "SELECT state FROM purpose_media_operations WHERE run_id=?1",
            [run],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    match state.as_deref() {
        None => Ok(CancelRecord::Missing),
        Some("accepted" | "cancelled" | "failed" | "unknown") => Ok(CancelRecord::AlreadyTerminal),
        Some("cancel_requested") => Ok(CancelRecord::Accepted),
        Some(_) => {
            db.execute(
                "UPDATE purpose_media_operations SET state='cancel_requested',updated_at=?2 WHERE run_id=?1 AND state NOT IN ('accepted','cancelled','failed','unknown','cancel_requested')",
                params![run, now],
            )
            .map_err(database_error)?;
            Ok(CancelRecord::Accepted)
        }
    }
}

/// Non-terminal rows from a previous process cannot be proven unsent.
pub fn reconcile_interrupted(db: &Connection, now: &str) -> Result<usize, String> {
    initialize(db)?;
    db.execute(
        "UPDATE purpose_media_operations SET state='unknown',updated_at=?1 WHERE state NOT IN ('accepted','cancelled','failed','unknown')",
        [now],
    )
    .map_err(database_error)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishOutcome {
    Accepted,
    Cancelled,
    Unknown,
    Failed,
}

pub fn release_unsent(db: &Connection, run: &str) -> Result<(), String> {
    db.execute(
        "DELETE FROM purpose_media_operations WHERE run_id=?1 AND remote_id IS NULL AND result_json IS NULL AND state NOT IN ('accepted','cancelled','failed','unknown','cancel_requested')",
        [run],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn finish(
    tx: &Transaction<'_>,
    now: &str,
    run: &str,
    route: &ResolvedRoute,
    result: &Result<MediaResult, MediaError>,
    validate: impl FnOnce(&Connection) -> Result<(), String>,
    accepted: impl FnOnce(&Connection, &ResolvedRoute) -> Result<(), String>,
) -> Result<FinishOutcome, String> {
    finish_in(tx, now, run, route, result, validate, accepted, false)
}

/// Explicit reconcile may replace an interrupted `unknown` row.
/// A late result from the original task still goes through `finish` and cannot.
pub fn finish_query(
    tx: &Transaction<'_>,
    now: &str,
    run: &str,
    route: &ResolvedRoute,
    result: &Result<MediaResult, MediaError>,
    validate: impl FnOnce(&Connection) -> Result<(), String>,
    accepted: impl FnOnce(&Connection, &ResolvedRoute) -> Result<(), String>,
) -> Result<FinishOutcome, String> {
    finish_in(tx, now, run, route, result, validate, accepted, true)
}

#[allow(clippy::too_many_arguments)]
fn finish_in(
    tx: &Transaction<'_>,
    now: &str,
    run: &str,
    route: &ResolvedRoute,
    result: &Result<MediaResult, MediaError>,
    validate: impl FnOnce(&Connection) -> Result<(), String>,
    accepted: impl FnOnce(&Connection, &ResolvedRoute) -> Result<(), String>,
    replace_unknown: bool,
) -> Result<FinishOutcome, String> {
    let current: String = tx
        .query_row(
            "SELECT state FROM purpose_media_operations WHERE run_id=?1",
            [run],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let replace_this_unknown = replace_unknown && current == "unknown";
    if let Some(outcome) = match current.as_str() {
        "accepted" => Some(FinishOutcome::Accepted),
        "cancelled" => Some(FinishOutcome::Cancelled),
        "unknown" if !replace_this_unknown => Some(FinishOutcome::Unknown),
        "failed" => Some(FinishOutcome::Failed),
        _ => None,
    } {
        if result.is_ok() && outcome != FinishOutcome::Accepted {
            validate(tx)?;
        }
        return Ok(outcome);
    }
    if current == "cancel_requested" && result.is_ok() {
        let error = MediaError {
            kind: saaa_larm_session::media::FailureKind::Cancelled,
            code: "cancelled_before_adoption".into(),
            retryable: false,
            may_have_generated: false,
            job_id: None,
        };
        let error_json = serde_json::to_string(&error).map_err(|error| error.to_string())?;
        let count = tx
            .execute(
                "UPDATE purpose_media_operations SET state='cancelled',result_json=NULL,error_json=?2,updated_at=?3 WHERE run_id=?1 AND state='cancel_requested'",
                params![run, error_json, now],
            )
            .map_err(database_error)?;
        if count != 1 {
            return Err("中止済みの生成結果を確定できません".into());
        }
        return Ok(FinishOutcome::Cancelled);
    }
    let (status, result_json, error_json, job, outcome) = match result {
        Ok(result) => {
            let current: String = tx
                .query_row(
                    "SELECT state FROM purpose_media_operations WHERE run_id=?1",
                    [run],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if current == "cancel_requested" || current == "cancelled" {
                return Err("中止要求済みの生成結果は採用できません".into());
            }
            validate(tx)?;
            let mut used = route.clone();
            used.model = result.model.clone();
            accepted(tx, &used)?;
            (
                "accepted",
                Some(serde_json::to_string(result).map_err(|error| error.to_string())?),
                None,
                result.job_id.as_deref(),
                FinishOutcome::Accepted,
            )
        }
        Err(error) => {
            let status = if error.kind == saaa_larm_session::media::FailureKind::Cancelled
                && !error.may_have_generated
            {
                "cancelled"
            } else if error.may_have_generated {
                "unknown"
            } else {
                "failed"
            };
            let outcome = match status {
                "cancelled" => FinishOutcome::Cancelled,
                "unknown" => FinishOutcome::Unknown,
                _ => FinishOutcome::Failed,
            };
            (
                status,
                None,
                Some(serde_json::to_string(error).map_err(|error| error.to_string())?),
                error.job_id.as_deref(),
                outcome,
            )
        }
    };
    let sql = if replace_this_unknown {
        "UPDATE purpose_media_operations SET state=?2,result_json=?3,error_json=?4,remote_id=COALESCE(?5,remote_id),updated_at=?6 WHERE run_id=?1 AND state='unknown'"
    } else {
        "UPDATE purpose_media_operations SET state=?2,result_json=?3,error_json=?4,remote_id=COALESCE(?5,remote_id),updated_at=?6 WHERE run_id=?1"
    };
    let count = tx
        .execute(sql, params![run, status, result_json, error_json, job, now])
        .map_err(database_error)?;
    if count != 1 {
        if replace_this_unknown {
            let preserved: Option<String> = tx
                .query_row(
                    "SELECT state FROM purpose_media_operations WHERE run_id=?1",
                    [run],
                    |row| row.get(0),
                )
                .optional()
                .map_err(database_error)?;
            return match preserved.as_deref() {
                Some("accepted") => Ok(FinishOutcome::Accepted),
                Some("cancelled") => Ok(FinishOutcome::Cancelled),
                Some("failed") => Ok(FinishOutcome::Failed),
                _ => Err("生成結果を記録できません".into()),
            };
        }
        return Err("生成結果を記録できません".into());
    }
    Ok(outcome)
}

pub fn get(db: &Connection, run: &str) -> Result<Option<Value>, String> {
    db.query_row(
        "SELECT kind,route_json,state,remote_id,result_json,error_json,updated_at FROM purpose_media_operations WHERE run_id=?1",
        [run],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        },
    )
    .optional()
    .map_err(database_error)?
    .map(|(kind, route, status, job, result, error, at)| row_value(StoredRow { run, kind: &kind, route: &route, status: &status, job, result, error, updated_at: &at, history_shape: false }))
    .transpose()
}

pub fn history(db: &Connection) -> Result<Vec<Value>, String> {
    history_limited(db, 20, None)
}

pub fn history_limited(
    db: &Connection,
    limit: u32,
    run: Option<&str>,
) -> Result<Vec<Value>, String> {
    let mut statement = db
        .prepare(
            "SELECT run_id,kind,route_json,state,remote_id,result_json,error_json,updated_at FROM purpose_media_operations WHERE (?1 IS NULL OR run_id=?1) ORDER BY updated_at DESC LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![run, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(database_error)?;
    rows.map(|row| {
        let (run, kind, route, status, job, result, error, at) = row.map_err(database_error)?;
        row_value(StoredRow {
            run: &run,
            kind: &kind,
            route: &route,
            status: &status,
            job,
            result,
            error,
            updated_at: &at,
            history_shape: true,
        })
    })
    .collect()
}

struct StoredRow<'a> {
    run: &'a str,
    kind: &'a str,
    route: &'a str,
    status: &'a str,
    job: Option<String>,
    result: Option<String>,
    error: Option<String>,
    updated_at: &'a str,
    history_shape: bool,
}

fn row_value(row: StoredRow<'_>) -> Result<Value, String> {
    let StoredRow {
        run,
        kind,
        route,
        status,
        job,
        result,
        error,
        updated_at: at,
        history_shape,
    } = row;
    let route: Value = serde_json::from_str(route).map_err(|error| error.to_string())?;
    let kind: Value = serde_json::from_str(kind).map_err(|error| error.to_string())?;
    let result = result
        .map(|value| serde_json::from_str::<Value>(&value))
        .transpose()
        .map_err(|error| error.to_string())?;
    let error = error
        .map(|value| serde_json::from_str::<Value>(&value))
        .transpose()
        .map_err(|error| error.to_string())?;
    if history_shape {
        Ok(json!({
            "runId": run,
            "kind": kind,
            "connectionLabel": route["connectionLabel"],
            "model": route["model"],
            "status": status,
            "jobId": job,
            "result": result,
            "error": error,
            "updatedAt": at
        }))
    } else {
        Ok(json!({
            "runId": run,
            "kind": kind,
            "route": route,
            "status": status,
            "jobId": job,
            "result": result,
            "error": error,
            "updatedAt": at
        }))
    }
}

pub fn cached(db: &Connection, run: &str, index: usize) -> Result<Option<Vec<u8>>, String> {
    db.query_row(
        "SELECT a.content FROM purpose_media_artifacts a JOIN purpose_media_operations o ON o.run_id=a.run_id WHERE a.run_id=?1 AND a.artifact_index=?2 AND o.state='accepted'",
        params![run, index],
        |row| row.get(0),
    )
    .optional()
    .map_err(database_error)
}

pub fn cache(db: &Connection, run: &str, index: usize, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("成果物が大きすぎます".into());
    }
    db.execute(
        "INSERT OR IGNORE INTO purpose_media_artifacts(run_id,artifact_index,content) VALUES(?1,?2,?3)",
        params![run, index, bytes],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn mark_absent_cancelled(db: &Connection, now: &str, run: &str) -> Result<(), String> {
    db.execute(
        "INSERT OR IGNORE INTO purpose_media_operations(run_id,kind,route_json,state,updated_at) VALUES(?1,'null','null','cancelled',?2)",
        params![run, now],
    )
    .map_err(database_error)?;
    Ok(())
}
