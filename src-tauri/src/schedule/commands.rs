use super::{
    calendar,
    contracts::{ScheduleAddInput, ScheduleEntryView, ScheduleStatus},
    forget, ledger,
    ledger::{Entry, Kind, Origin, Status},
    runtime, tick,
};
use crate::{validate_identifier, AppState};
use rusqlite::Connection;

#[tauri::command]
pub(crate) fn schedule_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ScheduleEntryView>, String> {
    state.sqlite_readers.read(list_views)
}

#[tauri::command]
pub(crate) fn schedule_add(
    state: tauri::State<'_, AppState>,
    input: ScheduleAddInput,
) -> Result<ScheduleEntryView, String> {
    let kind = parse_add_kind(&input.kind)?;
    validate_ref(&input.subject_ref, "subject ref")?;
    validate_ref(&input.scope_ref, "scope ref")?;
    if let Some(delegation) = input.delegation_ref.as_deref() {
        if !delegation.is_empty() {
            validate_ref(delegation, "delegation ref")?;
        }
    }
    if let Some(body) = input.payload.as_deref() {
        if body.len() > 8_000 {
            return Err("payload-too-large".into());
        }
    }
    if let Some(window_end_at) = input.window_end_at {
        if window_end_at < input.due_at {
            return Err("invalid-schedule-window".into());
        }
    }
    let now = tick::now_ms();
    state.sqlite_writer.write(|connection| {
        let payload_id = match input.payload.as_deref() {
            Some(body) if !body.is_empty() => {
                let id = crate::new_id("schp");
                ledger::insert_payload(connection, &id, Some(body), "internal")?;
                Some(id)
            }
            _ => None,
        };
        let entry = Entry {
            id: crate::new_id("sched"),
            kind,
            subject_ref: input.subject_ref.clone(),
            scope_ref: input.scope_ref.clone(),
            due_at: input.due_at,
            window_end_at: input.window_end_at,
            status: Status::Scheduled,
            origin: Origin::UserExplicit,
            delegation_ref: input
                .delegation_ref
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            revision: 1,
            supersedes: None,
            created_at: now,
            fired_at: None,
            fire_result: None,
            payload_id,
        };
        ledger::insert(connection, &entry)?;
        if let Ok(settings) = runtime::load(connection) {
            if let Some(calendar_id) = settings.calendar_id.as_deref() {
                if settings.calendar_enabled {
                    calendar::projection::enqueue_pending(connection, calendar_id, now)?;
                }
            }
        }
        view(&entry)
    })
}

#[tauri::command]
pub(crate) fn schedule_withdraw(
    state: tauri::State<'_, AppState>,
    id: String,
    revision: i64,
) -> Result<bool, String> {
    validate_identifier(&id, "schedule id")?;
    let now = tick::now_ms();
    state.sqlite_writer.write(|connection| {
        let withdrawn = ledger::withdraw(connection, &id, revision)?;
        if withdrawn {
            calendar::projection::mark_stale(connection, &id, now)?;
        }
        Ok(withdrawn)
    })
}

#[tauri::command]
pub(crate) fn schedule_status(state: tauri::State<'_, AppState>) -> Result<ScheduleStatus, String> {
    let settings = state.sqlite_writer.read_serialized(runtime::load)?;
    let allow_legacy = calendar::auth::legacy_entry_is_unambiguous(&state);
    Ok(ScheduleStatus {
        enabled: settings.enabled,
        calendar_enabled: settings.calendar_enabled,
        calendar_id: settings.calendar_id,
        calendar_connected: calendar::auth::load_refresh(&state.schedule, allow_legacy)
            .ok()
            .flatten()
            .is_some()
            || state.schedule.access().is_some(),
        last_error: settings.last_error,
        platform_supported: cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )),
        oauth_client_id: settings.oauth_client_id,
    })
}

#[tauri::command]
pub(crate) fn schedule_set_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<ScheduleStatus, String> {
    let now = tick::now_ms();
    if enabled {
        tick::enable(&state, now)?;
    } else {
        tick::disable(&state, now)?;
    }
    schedule_status(state)
}

#[tauri::command]
pub(crate) fn schedule_set_calendar(
    state: tauri::State<'_, AppState>,
    enabled: bool,
    calendar_id: Option<String>,
) -> Result<ScheduleStatus, String> {
    if enabled {
        calendar::auth::unsupported()?;
    }
    if let Some(calendar_id) = calendar_id.as_deref() {
        if calendar_id.len() > 160 {
            return Err("Invalid calendar id".into());
        }
    }
    state.sqlite_writer.write(|connection| {
        runtime::set_calendar(connection, enabled, calendar_id.as_deref())?;
        Ok(())
    })?;
    state
        .schedule
        .set_calendar_ready(enabled && calendar_id.is_some());
    if !enabled {
        let delete_legacy = calendar::auth::legacy_entry_is_unambiguous(&state);
        calendar::auth::clear(&state.schedule, delete_legacy)?;
    }
    schedule_status(state)
}

#[tauri::command]
pub(crate) async fn schedule_connect_calendar(
    state: tauri::State<'_, AppState>,
    client_id: Option<String>,
) -> Result<ScheduleStatus, String> {
    match calendar::oauth::connect(&state, client_id).await {
        Ok(()) => {
            let _ = state
                .sqlite_writer
                .write(|connection| runtime::set_error(connection, None));
        }
        Err(error) => {
            let _ = state
                .sqlite_writer
                .write(|connection| runtime::set_error(connection, Some("oauth")));
            return Err(error);
        }
    }
    schedule_status(state)
}

#[tauri::command]
pub(crate) fn schedule_disconnect_calendar(
    state: tauri::State<'_, AppState>,
) -> Result<ScheduleStatus, String> {
    let delete_legacy = calendar::auth::legacy_entry_is_unambiguous(&state);
    calendar::auth::clear(&state.schedule, delete_legacy)?;
    schedule_status(state)
}

#[tauri::command]
pub(crate) fn schedule_forget(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    validate_identifier(&id, "forget id")?;
    let now = tick::now_ms();
    state
        .sqlite_writer
        .write(|connection| forget::forget(connection, &id, now))
}

fn parse_add_kind(kind: &str) -> Result<Kind, String> {
    match kind {
        "task_run" => Ok(Kind::TaskRun),
        "check_in" => Ok(Kind::CheckIn),
        "digest" => Ok(Kind::Digest),
        "reminder" => Ok(Kind::Reminder),
        _ => Err("invalid-schedule-kind".into()),
    }
}

fn validate_ref(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || value.starts_with(':')
        || value.ends_with(':')
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        return Err(format!("Invalid {label}"));
    }
    Ok(())
}

fn list_views(connection: &Connection) -> Result<Vec<ScheduleEntryView>, String> {
    ledger::list_all(connection, 200)?
        .into_iter()
        .map(|entry| view(&entry))
        .collect()
}

fn view(entry: &Entry) -> Result<ScheduleEntryView, String> {
    Ok(ScheduleEntryView {
        id: entry.id.clone(),
        kind: entry.kind.as_str().into(),
        subject_ref: entry.subject_ref.clone(),
        scope_ref: entry.scope_ref.clone(),
        due_at: entry.due_at,
        status: entry.status.as_str().into(),
        origin: entry.origin.as_str().into(),
        revision: entry.revision,
        fire_result: entry.fire_result.as_ref().map(|value| value.as_str()),
        payload_id: entry.payload_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_add_kind, validate_ref};

    #[test]
    fn sl_10_rejects_invalid_kind_and_refs() {
        assert!(parse_add_kind("hold_until").is_err());
        assert!(validate_ref("", "subject ref").is_err());
        assert!(validate_ref("../secret", "subject ref").is_err());
        assert!(validate_ref("goal:ok", "subject ref").is_ok());
        assert!(parse_add_kind("reminder").is_ok());
    }
}
