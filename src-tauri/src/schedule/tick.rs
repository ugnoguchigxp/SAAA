use super::{
    calendar, decide, hold, ledger,
    ledger::{Entry, FireResult, Kind, Status},
    notify, runtime,
};
use crate::{
    database_error, memory::personal_state::worker::generation_slot_busy, new_id,
    situation::speech_holds_tts, AppState,
};
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;

pub(crate) const TICK_MS: u64 = 45_000;
const DUE_LIMIT: i64 = 32;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn situation_hold(state: &AppState) -> bool {
    speech_holds_tts(state)
}

pub(crate) fn tick(state: &AppState, now: i64) -> Result<usize, String> {
    let settings = state.sqlite_writer.read_serialized(runtime::load)?;
    if !settings.enabled {
        return Ok(0);
    }
    let hold = situation_hold(state);
    let busy = generation_slot_busy();
    let (fired, pending_dispatches) = state.sqlite_writer.write(|connection| {
        let tx = connection.unchecked_transaction().map_err(database_error)?;
        recover_if_needed(&tx, now)?;
        ledger::mark_missed(&tx, now)?;
        let due = ledger::due(&tx, now, DUE_LIMIT)?;
        let mut count = 0;
        let mut pending_dispatches = Vec::new();
        for entry in due {
            if matches!(entry.kind, Kind::HoldUntil) {
                continue;
            }
            let (fired_one, dispatch) = fire_one(state, &tx, &entry, now, hold, busy)?;
            if fired_one {
                count += 1;
            }
            if let Some(dispatch) = dispatch {
                pending_dispatches.push(dispatch);
            }
        }
        if !hold {
            flush_digest(&tx, now)?;
        }
        if settings.calendar_enabled {
            if let Some(calendar_id) = settings.calendar_id.as_deref() {
                calendar::projection::enqueue_pending(&tx, calendar_id, now)?;
            }
        }
        tx.execute(
            "UPDATE schedule_runtime SET last_tick_at=?1 WHERE id=1",
            [now],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok((count, pending_dispatches))
    })?;
    // Dispatch starts only after the schedule Firing marker is durable.  The
    // Coding service persists the steward intent/receipt before Pi is opened;
    // a crash here therefore recovers as inspection/unknown, never a replay.
    for entry in pending_dispatches {
        finish_task_dispatch(state, &entry, now)?;
    }
    // This is a delivery wake-up only.  It never creates or dispatches work,
    // and Situation hold remains the final gate for conversation delivery.
    if !hold {
        let _ = crate::steward::flush_all_held_reports(state);
    }
    if settings.calendar_enabled {
        let _ = calendar::projection::flush(state, now);
        let _ = calendar::observe::sync(state, now);
    }
    Ok(fired)
}

pub(crate) fn recover_if_needed(connection: &Connection, now: i64) -> Result<(), String> {
    let firing: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schedule_entries WHERE status='firing')",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if firing == 0 {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT kind, subject_ref FROM schedule_entries WHERE status='firing'")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(crate::database_error)?;
    let mut inspect = Vec::new();
    for row in rows {
        let (kind, subject) = row.map_err(crate::database_error)?;
        if kind == Kind::TaskRun.as_str() {
            inspect.push(subject);
        }
    }
    drop(statement);
    for subject in inspect {
        let _ = notify::assistant(connection, &notify::inspect(&subject), now);
    }
    ledger::close_firing(connection, "crashed", now)?;
    Ok(())
}

fn fire_one(
    state: &AppState,
    connection: &Connection,
    entry: &Entry,
    now: i64,
    hold: bool,
    busy: bool,
) -> Result<(bool, Option<Entry>), String> {
    if !ledger::cas_status(
        connection,
        &entry.id,
        entry.revision,
        Status::Scheduled,
        Status::Firing,
        None,
        None,
    )? {
        return Ok((false, None));
    }
    let decision = decide::decide(entry, hold, busy);
    if matches!(entry.kind, Kind::TaskRun) && matches!(decision, decide::Decision::Act) {
        return Ok((true, Some(entry.clone())));
    }
    let result = decide::fire_result(&decision);
    match &decision {
        decide::Decision::Act => {
            notify::assistant(connection, &notify::started(&entry.subject_ref), now)?;
            state.schedule.record_action(&entry.id);
        }
        decide::Decision::Hold => {
            hold::enqueue_hold(connection, entry, now)?;
        }
        decide::Decision::Defer => {
            enqueue_retry(connection, entry, now)?;
        }
        decide::Decision::Ask => {
            notify::assistant(connection, &notify::candidate(&entry.subject_ref), now)?;
        }
    }
    if !ledger::cas_status(
        connection,
        &entry.id,
        entry.revision,
        Status::Firing,
        Status::Fired,
        Some(&result),
        Some(now),
    )? {
        return Err("schedule-fire-cas-failed".into());
    }
    if state.schedule.calendar_ready() {
        calendar::projection::mark_stale(connection, &entry.id, now)?;
    }
    Ok((true, None))
}

fn finish_task_dispatch(state: &AppState, entry: &Entry, now: i64) -> Result<(), String> {
    let task_id = entry
        .subject_ref
        .strip_prefix("task:")
        .ok_or("schedule_task_ref_invalid")?;
    let delegation_id = entry
        .delegation_ref
        .as_deref()
        .ok_or("schedule_delegation_missing")?;
    let dispatched = crate::steward::dispatch_scheduled(state, task_id, delegation_id);
    state.sqlite_writer.write(|connection| {
        let (result, notify_started) = match dispatched {
            Ok(_) => (FireResult::Started, true),
            Err(_) => (FireResult::NoDelegation, false),
        };
        if notify_started {
            notify::assistant(connection, &notify::started(&entry.subject_ref), now)?;
            state.schedule.record_action(&entry.id);
        } else {
            notify::assistant(connection, &notify::candidate(&entry.subject_ref), now)?;
        }
        if !ledger::cas_status(
            connection,
            &entry.id,
            entry.revision,
            Status::Firing,
            Status::Fired,
            Some(&result),
            Some(now),
        )? {
            return Err("schedule-task-dispatch-cas-failed".into());
        }
        if state.schedule.calendar_ready() {
            calendar::projection::mark_stale(connection, &entry.id, now)?;
        }
        Ok(())
    })
}

fn enqueue_retry(connection: &Connection, source: &Entry, now: i64) -> Result<(), String> {
    let mut retry = source.clone();
    retry.id = new_id("sched");
    retry.due_at = now.saturating_add(TICK_MS as i64);
    retry.window_end_at = source.window_end_at.map(|end| end.max(retry.due_at));
    retry.status = Status::Scheduled;
    retry.revision = 1;
    retry.supersedes = None;
    retry.created_at = now;
    retry.fired_at = None;
    retry.fire_result = None;
    ledger::insert(connection, &retry)
}

fn flush_digest(connection: &Connection, now: i64) -> Result<(), String> {
    let subjects = hold::pending_hold_subjects(connection, now)?;
    if subjects.is_empty() {
        return Ok(());
    }
    let mut statement = connection
        .prepare(
            "SELECT id, revision FROM schedule_entries
             WHERE kind='hold_until' AND status='scheduled' AND due_at<=?1",
        )
        .map_err(crate::database_error)?;
    let rows = statement
        .query_map([now], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(crate::database_error)?;
    let holds = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    drop(statement);
    let started = FireResult::Started;
    let mut closed = 0;
    for (id, revision) in holds {
        if ledger::cas_status(
            connection,
            &id,
            revision,
            Status::Scheduled,
            Status::Firing,
            None,
            None,
        )? {
            if !ledger::cas_status(
                connection,
                &id,
                revision,
                Status::Firing,
                Status::Fired,
                Some(&started),
                Some(now),
            )? {
                return Err("schedule-hold-cas-failed".into());
            }
            closed += 1;
        }
    }
    if closed > 0 {
        hold::record_digest(connection, crate::PRIMARY_CONVERSATION_ID, &subjects, now)?;
    }
    Ok(())
}

pub(crate) fn disable(state: &AppState, now: i64) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        runtime::set_enabled(connection, false)?;
        ledger::close_firing(connection, "disabled", now)?;
        Ok(())
    })?;
    state.schedule.set_enabled(false);
    Ok(())
}

pub(crate) fn enable(state: &AppState, now: i64) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
        runtime::set_enabled(connection, true)?;
        recover_if_needed(connection, now)?;
        Ok(())
    })?;
    state.schedule.set_enabled(true);
    tick(state, now)?;
    Ok(())
}

pub(crate) fn start_loop(app: tauri::AppHandle) {
    {
        let state = app.state::<crate::AppState>();
        state.steward_wake.bind(app.clone());
    }
    tauri::async_runtime::spawn(async move {
        {
            let state = app.state::<crate::AppState>();
            state.steward_wake.install_process();
            let _ = crate::steward::pump::drain(&state);
            if state.schedule.enabled() {
                let _ = tick(&state, now_ms());
            }
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(TICK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            let state = app.state::<crate::AppState>();
            if state
                .shutdown_started
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            let due = state.steward_wake.next_due_ms(&state);
            let wait = due.map(|at| {
                let now = now_ms();
                std::time::Duration::from_millis(
                    at.saturating_sub(now).clamp(1, TICK_MS as i64) as u64
                )
            });
            tokio::select! {
                _ = interval.tick() => {
                    if state.schedule.enabled() {
                        let _ = tick(&state, now_ms());
                    }
                    let _ = crate::steward::pump::drain(&state);
                }
                _ = state.steward_wake.notified() => {
                    let _ = crate::steward::pump::drain(&state);
                }
                _ = async {
                    if let Some(duration) = wait {
                        tokio::time::sleep(duration).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let _ = crate::steward::pump::drain(&state);
                }
            }
        }
    });
}
