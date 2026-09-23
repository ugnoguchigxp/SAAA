use super::*;
pub(super) fn state() -> AppState {
    let connection = Connection::open_in_memory().unwrap();
    initialize_database(&connection).unwrap();
    app_state(connection)
}
pub(super) fn insert_due(
    state: &AppState,
    id: &str,
    due_at: i64,
    delegation: Option<&str>,
    window: Option<i64>,
) {
    let entry = Entry {
        id: id.into(),
        kind: Kind::TaskRun,
        subject_ref: "task:t1".into(),
        scope_ref: "scope:primary".into(),
        due_at,
        window_end_at: window,
        status: Status::Scheduled,
        origin: Origin::Delegation,
        delegation_ref: delegation.map(str::to_string),
        revision: 1,
        supersedes: None,
        created_at: 1,
        fired_at: None,
        fire_result: None,
        payload_id: None,
    };
    state
        .sqlite_writer
        .write(|connection| ledger::insert(connection, &entry))
        .unwrap();
}
pub(super) fn status_of(state: &AppState, id: &str) -> (Status, Option<FireResult>) {
    state
        .sqlite_writer
        .read_serialized(|connection| {
            let entry = ledger::get(connection, id)?.expect("entry");
            Ok((entry.status, entry.fire_result))
        })
        .unwrap()
}
pub(super) fn messages(state: &AppState) -> Vec<String> {
    state
        .sqlite_writer
        .read_serialized(|connection| {
            let mut statement = connection
                .prepare("SELECT content FROM conversation_messages ORDER BY created_at, id")
                .map_err(crate::database_error)?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(crate::database_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(crate::database_error)
        })
        .unwrap()
}
#[test]
pub(super) fn sl_05_task_run_without_dispatch_receipt_is_not_started() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "a1", 50, Some("del-1"), None);
    assert_eq!(tick::tick(&state, 100).unwrap(), 1);
    assert_eq!(tick::tick(&state, 101).unwrap(), 0);
    let (status, result) = status_of(&state, "a1");
    assert_eq!(status, Status::Fired);
    assert_eq!(result, Some(FireResult::NoDelegation));
    assert!(state.schedule.actions().is_empty());
    assert_eq!(
        messages(&state)
            .iter()
            .filter(|value| value.starts_with("schedule-started:"))
            .count(),
        0
    );
}
#[test]
pub(super) fn sl_07_b_asks_without_delegation() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "b1", 50, None, None);
    tick::tick(&state, 100).unwrap();
    assert!(state.schedule.actions().is_empty());
    assert_eq!(status_of(&state, "b1").1, Some(FireResult::NoDelegation));
    assert!(messages(&state)
        .iter()
        .any(|value| value == "schedule-ask:task:t1"));
}
#[test]
pub(super) fn sl_08_c_holds_on_situation_then_digest() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    state
        .situation
        .set_scene_attention_for_test("MEETING", "OBSERVE");
    insert_due(&state, "c1", 50, Some("del-1"), None);
    tick::tick(&state, 100).unwrap();
    assert!(state.schedule.actions().is_empty());
    assert_eq!(status_of(&state, "c1").1, Some(FireResult::Deferred));
    state
        .situation
        .set_scene_attention_for_test("FOCUS", "RESPOND");
    tick::tick(&state, 120).unwrap();
    assert_eq!(
        messages(&state)
            .iter()
            .filter(|value| value.starts_with("schedule-digest:"))
            .count(),
        1
    );
}
#[test]
pub(super) fn sl_05_d_missed_after_window() {
    let state = state();
    tick::enable(&state, 200).unwrap();
    insert_due(&state, "d1", 50, Some("del-1"), Some(80));
    tick::tick(&state, 200).unwrap();
    assert_eq!(status_of(&state, "d1").0, Status::Missed);
    assert!(state.schedule.actions().is_empty());
}
#[test]
pub(super) fn sl_06_e_crash_closes_firing_without_rerun() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "e1", 50, Some("del-1"), None);
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE schedule_entries SET status='firing' WHERE id='e1'",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    tick::tick(&state, 100).unwrap();
    assert_eq!(
        status_of(&state, "e1").1,
        Some(FireResult::Error("crashed".into()))
    );
    assert!(state.schedule.actions().is_empty());
    assert!(messages(&state)
        .iter()
        .any(|value| value.starts_with("schedule-inspect:")));
}
#[test]
pub(super) fn sl_06_reminder_firing_is_closed_without_inspect() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "r1", 50, Some("del-1"), None);
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE schedule_entries SET status='firing', kind='reminder' WHERE id='r1'",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    tick::tick(&state, 100).unwrap();
    assert_eq!(
        status_of(&state, "r1").1,
        Some(FireResult::Error("crashed".into()))
    );
    assert!(messages(&state)
        .iter()
        .all(|value| !value.starts_with("schedule-inspect:")));
}
#[test]
pub(super) fn sl_05_f_time_change_supersedes_old() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "old", 50, Some("del-1"), None);
    state
        .sqlite_writer
        .write(|connection| {
            let old = ledger::get(connection, "old")?.unwrap();
            let mut next = old.clone();
            next.id = "new".into();
            next.due_at = 500;
            next.supersedes = Some("old".into());
            next.revision = 2;
            ledger::supersede(connection, &old, &next)
        })
        .unwrap();
    tick::tick(&state, 100).unwrap();
    assert_eq!(status_of(&state, "old").0, Status::Superseded);
    assert_eq!(status_of(&state, "new").0, Status::Scheduled);
    assert!(state.schedule.actions().is_empty());
}
#[test]
pub(super) fn sl_05_g_withdrawn_does_not_fire() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "g1", 50, Some("del-1"), None);
    state
        .sqlite_writer
        .write(|connection| ledger::withdraw(connection, "g1", 1))
        .unwrap();
    tick::tick(&state, 100).unwrap();
    assert_eq!(status_of(&state, "g1").0, Status::Withdrawn);
    assert!(state.schedule.actions().is_empty());
}
#[test]
pub(super) fn sl_09_defers_when_generation_slot_busy() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "busy", 50, Some("del-1"), None);
    let _guard = crate::memory::personal_state::worker::occupy_for_test();
    tick::tick(&state, 100).unwrap();
    assert!(state.schedule.actions().is_empty());
    assert_eq!(status_of(&state, "busy").1, Some(FireResult::Deferred));
    assert!(!messages(&state)
        .iter()
        .any(|content| content.starts_with("schedule-digest")));
    let retries: i64 = state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM schedule_entries
                     WHERE status='scheduled' AND due_at>100 AND kind='task_run'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(retries, 1);
}
#[test]
pub(super) fn sl_21_r_disable_stops_tick_and_closes_firing() {
    let state = state();
    tick::enable(&state, 100).unwrap();
    insert_due(&state, "r1", 50, Some("del-1"), None);
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE schedule_entries SET status='firing' WHERE id='r1'",
                    [],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    tick::disable(&state, 100).unwrap();
    assert!(!state.schedule.enabled());
    assert_eq!(
        status_of(&state, "r1").1,
        Some(FireResult::Error("disabled".into()))
    );
    assert_eq!(tick::tick(&state, 200).unwrap(), 0);
}
#[test]
pub(super) fn sl_04_due_p95_under_five_ms() {
    let connection = Connection::open_in_memory().unwrap();
    super::super::schema::migrate(&connection).unwrap();
    for index in 0..1000 {
        let mut entry = Entry {
            id: format!("p{index}"),
            kind: Kind::Reminder,
            subject_ref: "goal:g".into(),
            scope_ref: "scope:primary".into(),
            due_at: i64::from(index),
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
        if index >= 32 {
            entry.due_at = 10_000;
        }
        ledger::insert(&connection, &entry).unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let due = ledger::due(&connection, 31, 32).unwrap();
        samples.push(started.elapsed());
        assert_eq!(due.len(), 32);
    }
    samples.sort();
    assert!(samples[18].as_millis() <= 5, "p95 {:?}", samples[18]);
}
#[test]
pub(super) fn sl_23_tick_p95_under_fifteen_ms() {
    let state = state();
    tick::enable(&state, 0).unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            for index in 0..1000 {
                let due = if index < 32 { i64::from(index) } else { 50_000 };
                ledger::insert(
                    connection,
                    &Entry {
                        id: format!("t{index}"),
                        kind: Kind::Reminder,
                        subject_ref: "goal:g".into(),
                        scope_ref: "scope:primary".into(),
                        due_at: due,
                        window_end_at: None,
                        status: Status::Scheduled,
                        origin: Origin::UserExplicit,
                        delegation_ref: Some("d".into()),
                        revision: 1,
                        supersedes: None,
                        created_at: 1,
                        fired_at: None,
                        fire_result: None,
                        payload_id: None,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut samples = Vec::new();
    for _ in 0..8 {
        let started = Instant::now();
        tick::tick(&state, 40).unwrap();
        samples.push(started.elapsed());
    }
    samples.sort();
    assert!(samples[6].as_millis() <= 15, "p95 {:?}", samples[6]);
}
#[test]
pub(super) fn sl_19_o_forget_clears_payload_and_summary() {
    let state = state();
    state
        .sqlite_writer
        .write(|connection| {
            ledger::insert_payload(connection, "schp_1", Some("secret-body"), "internal")?;
            connection
                .execute(
                    "INSERT INTO calendar_observations(
                       id,event_id,observed_at,remote_etag,remote_summary,diff_kind,handled
                     ) VALUES('scho_1','ev',1,'etag','secret-title','unchanged','ignored')",
                    [],
                )
                .map_err(crate::database_error)?;
            forget::forget(connection, "schp_1", 2)?;
            forget::forget(connection, "scho_1", 2)?;
            Ok(())
        })
        .unwrap();
    let (body, summary): (Option<String>, Option<String>) = state
        .sqlite_writer
        .read_serialized(|connection| {
            Ok((
                connection
                    .query_row(
                        "SELECT body FROM schedule_payloads WHERE id='schp_1'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)?,
                connection
                    .query_row(
                        "SELECT remote_summary FROM calendar_observations WHERE id='scho_1'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::database_error)?,
            ))
        })
        .unwrap();
    assert!(body.is_none());
    assert!(summary.is_none());
    let forgotten = state
        .sqlite_writer
        .read_serialized(|connection| forget::forgotten(connection, "schp_1"))
        .unwrap();
    assert!(forgotten);
}
#[test]
pub(super) fn sl_20_s_restricted_title_is_reference_only() {
    let entry = Entry {
        id: "s1".into(),
        kind: Kind::Reminder,
        subject_ref: "goal:private".into(),
        scope_ref: "scope:primary".into(),
        due_at: 1,
        window_end_at: None,
        status: Status::Scheduled,
        origin: Origin::UserExplicit,
        delegation_ref: None,
        revision: 1,
        supersedes: None,
        created_at: 1,
        fired_at: None,
        fire_result: None,
        payload_id: Some("schp_s".into()),
    };
    let title = calendar::encode::event_title(&entry, Some("secret-text"), "confidential");
    assert_eq!(title, "goal:private");
    assert!(!title.contains("secret"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub(super) async fn sl_23_h_i_n_p_calendar_fake_http() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let calls = std::sync::Arc::new(std::sync::Mutex::new(0_u32));
    let events_server = events.clone();
    let calls_server = calls.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut bytes = Vec::new();
            let mut buffer = [0; 8192];
            if let Ok(count) = socket.read(&mut buffer).await {
                bytes.extend_from_slice(&buffer[..count]);
            }
            let header = String::from_utf8_lossy(&bytes);
            *calls_server.lock().unwrap() += 1;
            let body = if header.starts_with("POST ") {
                let mut events = events_server.lock().unwrap();
                if events.is_empty() {
                    let event = serde_json::json!({
                        "id": "evt-1",
                        "etag": "etag-1",
                        "status": "confirmed",
                        "summary": "task:t1",
                        "start": { "dateTime": "1970-01-01T00:00:00.050+00:00" },
                        "extendedProperties": { "private": { "saaa_entry_id": "h1", "saaa_rev": "1", "saaa_hash": "x" } }
                    });
                    events.push(event.clone());
                    event.to_string()
                } else {
                    events[0].to_string()
                }
            } else if header.contains("privateExtendedProperty") {
                let events = events_server.lock().unwrap();
                serde_json::json!({ "items": *events }).to_string()
            } else if header.starts_with("GET ") {
                let events = events_server.lock().unwrap();
                serde_json::json!({ "items": *events, "nextSyncToken": "tok-1" }).to_string()
            } else {
                serde_json::json!({ "id": "evt-1", "etag": "etag-2", "status": "confirmed",
                    "extendedProperties": { "private": { "saaa_entry_id": "h1", "saaa_rev": "1" } } })
                    .to_string()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let state = state();
    tick::enable(&state, 100).unwrap();
    state
        .schedule
        .set_http_base(format!("http://127.0.0.1:{}", address.port()));
    state.schedule.set_access("token");
    state.schedule.set_calendar_ready(true);
    state
        .sqlite_writer
        .write(|connection| {
            runtime::set_calendar(connection, true, Some("primary"))?;
            ledger::insert(
                connection,
                &Entry {
                    id: "h1".into(),
                    kind: Kind::Reminder,
                    subject_ref: "task:t1".into(),
                    scope_ref: "scope:primary".into(),
                    due_at: 50,
                    window_end_at: None,
                    status: Status::Scheduled,
                    origin: Origin::UserExplicit,
                    delegation_ref: Some("d".into()),
                    revision: 1,
                    supersedes: None,
                    created_at: 1,
                    fired_at: None,
                    fire_result: None,
                    payload_id: None,
                },
            )?;
            calendar::projection::enqueue_pending(connection, "primary", 100)?;
            Ok(())
        })
        .unwrap();
    calendar::projection::flush(&state, 100).unwrap();
    calendar::projection::flush(&state, 101).unwrap();
    calendar::observe::sync(&state, 102).unwrap();
    let stored: i64 = state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM calendar_projections WHERE event_id='evt-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(stored, 1);
    assert!(!state.schedule.api_calls().is_empty());
}
