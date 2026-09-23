use super::{contracts::CodingSettings, repository as repo};
use crate::{database_error, persistence::SqliteWriter, RunCancellation};
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Runs a coding job through Codex directly. Pi settings and sessions are never read here.
pub(crate) fn execute(
    writer: &SqliteWriter,
    run: &str,
    settings: &CodingSettings,
) -> Result<Value, String> {
    if !super::contracts::valid_implementation(settings)
        || settings.implementation_method != "codex-sdk"
    {
        return Err("coding_configuration_invalid".into());
    }
    let (job, workspace, payload, thread_id): (String, String, String, Option<String>) =
        writer.read_serialized(|connection| {
            connection.query_row(
                "SELECT j.id,j.workspace_path,r.payload,j.session_id FROM coding_runs r JOIN coding_jobs j ON j.id=r.job_id WHERE r.id=?1",
                [run],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).map_err(database_error)
        })?;
    let workspace = PathBuf::from(workspace);
    if std::fs::canonicalize(&workspace).map_err(|_| "workspace_missing")? != workspace
        || !workspace.join(".git").exists()
    {
        return Err("workspace_changed".into());
    }
    writer.write(|connection| {
        if !crate::runtime::pi::runner::source_valid(connection, run)? {
            return Err("source_unavailable".into());
        }
        let updated = connection.execute(
            "UPDATE coding_runs SET delivery='sending',process_identity='codex-app-server' WHERE id=?1 AND state='starting'",
            [run],
        ).map_err(database_error)?;
        if updated != 1 { return Err("cancelled_before_send".into()); }
        repo::event(connection, &job, run, "dispatching", json!({"method":"codex-sdk"}))
    })?;

    let cancellation = RunCancellation::default();
    let finished = AtomicBool::new(false);
    let sink = tauri::ipc::Channel::<crate::ipc_contract::RuntimeEvent>::new(|_| Ok(()));
    let policy = crate::runtime::contracts::RunSupervisionPolicy {
        request_timeout_ms: 20_000,
        progress_idle_timeout_ms: 60_000,
        terminal_gap_timeout_ms: 10_000,
        interrupt_grace_ms: 3_000,
        hard_timeout_ms: 1_800_000,
    };
    let outcome = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(200));
                let should_cancel = writer
                    .read_serialized(|connection| {
                        let state: String = connection
                            .query_row("SELECT state FROM coding_runs WHERE id=?1", [run], |row| {
                                row.get(0)
                            })
                            .map_err(database_error)?;
                        Ok(state == "stopping"
                            || !crate::runtime::pi::runner::source_valid(connection, run)?)
                    })
                    .unwrap_or(true);
                if should_cancel {
                    cancellation.cancel();
                    break;
                }
            }
        });
        let result = crate::runtime::codex_process::run_codex_turn_process_with_dispatch(
            run,
            &payload,
            &workspace,
            &settings.codex_model,
            thread_id.as_deref(),
            "",
            policy,
            &sink,
            &cancellation,
            None,
            true,
        );
        finished.store(true, Ordering::Release);
        let _ = watcher.join();
        result
    })
    .map_err(|failure| format!("{}: {}", failure.code.as_str(), failure.message))?;

    writer.write(|connection| {
        connection.execute(
            "UPDATE coding_jobs SET session_id=?2 WHERE id=?1",
            params![job, outcome.thread_id],
        ).map_err(database_error)?;
        connection.execute(
            "UPDATE coding_runs SET delivery='accepted',state=CASE WHEN state='stopping' THEN state ELSE 'running' END WHERE id=?1",
            [run],
        ).map_err(database_error)?;
        repo::event(connection, &job, run, "accepted", json!({"method":"codex-sdk"}))
    })?;
    Ok(json!({
        "summary": outcome.content,
        "errors": 0,
        "modelErrors": 0,
        "tools": [],
        "lastEntryId": outcome.thread_id,
        "sessionId": outcome.thread_id,
        "complete": true,
        "truncated": false,
        "meaning": "Codex turn ended; goal achievement is not certified"
    }))
}
