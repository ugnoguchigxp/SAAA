use super::*;
pub(crate) static ADAPTER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(unix)]
#[test]
pub(super) fn coding_service_runs_and_resumes_through_production_adapter_with_protocol_fixture() {
    run_adapter(false, false);
}
/// A source forget removes the originating conversation message.  The runner
/// must notice that durable authorization has disappeared, abort its live Pi
/// process, and expose the terminal outcome as an interruption.
#[cfg(unix)]
#[test]
pub(super) fn coding_service_source_forget_interrupts_fixture_process() {
    run_adapter(false, true);
}
#[cfg(unix)]
#[test]
#[ignore = "requires live pi and authenticated Codex SDK"]
pub(super) fn coding_service_codex_sdk_live() {
    run_adapter(true, false);
}
/// A delegated execution has no fabricated user turn.  Its durable steward
/// task is the only origin, and the shared Coding service records that origin
/// before the fixture Pi process is launched.
#[cfg(unix)]
#[test]
pub(super) fn coding_service_runs_through_a_delegated_event_origin() {
    let _lock = ADAPTER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("fixture-pi");
    std::fs::write(&executable, include_str!("../../test_pi.py")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(&workspace)
        .status()
        .unwrap()
        .success());
    let connection = database();
    let mut state = crate::test_support::app_state(connection);
    state.data_directory = directory.path().to_owned();
    let settings = contracts::CodingSettings {
        enabled: true,
        executable: executable.to_string_lossy().into_owned(),
        ..Default::default()
    };
    state
        .sqlite_writer
        .write(|c| {
            c.execute(
                "UPDATE coding_settings SET value_json=?1",
                [serde_json::to_string(&settings).unwrap()],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let registered = service::register(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        workspace.to_str().unwrap(),
    )
    .unwrap();
    let workspace_id = registered["workspaceId"].as_str().unwrap();
    state
        .sqlite_writer
        .write(|c| {
            c.execute(
                "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at)
                 VALUES('goal',?1,'user_explicit','test_report_obtained','active','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES('source',?1,'user','inspect the failures','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at)
                 VALUES('delegation','goal',?1,?2,'read_test',1,60000,'silent','active','1')",
                params![crate::PRIMARY_CONVERSATION_ID, workspace_id],
            )
            .map_err(crate::database_error)?;
            c.execute(
                "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
                 VALUES('task','delegation',?1,'start','source','delegation:source','queued','1','1')",
                [crate::PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let user_messages_before: i64 = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE role='user'",
                [],
                |row| row.get(0),
            )
            .map_err(crate::database_error)
        })
        .unwrap();
    let accepted = service::execute_delegated(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        "task",
        workspace_id,
        "inspect",
    )
    .unwrap();
    let job = accepted["jobId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let settled = loop {
        let current = state
            .sqlite_readers
            .read(|c| repo::inspect(c, crate::PRIMARY_CONVERSATION_ID, job, 0, 1))
            .unwrap();
        if current["state"] == "settled" {
            break current;
        }
        assert!(Instant::now() < deadline, "{current}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(settled["result"]["summary"], "fixture result 2");
    let (origin_kind, origin_id, user_messages): (String, String, i64) = state
        .sqlite_readers
        .read(|c| {
            Ok((
                c.query_row(
                    "SELECT origin_kind FROM coding_origin_bindings WHERE job_id=?1",
                    [job],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)?,
                c.query_row(
                    "SELECT origin_id FROM coding_origin_bindings WHERE job_id=?1",
                    [job],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)?,
                c.query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE role='user'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)?,
            ))
        })
        .unwrap();
    assert_eq!(
        (origin_kind.as_str(), origin_id.as_str()),
        ("delegated_event", "task")
    );
    // The user source already existed before dispatch. The delegated service
    // did not manufacture another StartTurnInput/message.
    assert_eq!(user_messages, user_messages_before);
}
/// Delegated Goals may queue independently, but the shared Coding service owns
/// one process slot. A queued sibling must not obtain a second Pi process while
/// the first task has an accepted, running effect.
#[cfg(unix)]
#[test]
pub(super) fn delegated_tasks_compete_for_one_production_coding_slot() {
    let _lock = ADAPTER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("fixture-pi");
    std::fs::write(&executable, include_str!("../../test_pi.py")).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(&workspace)
        .status()
        .unwrap()
        .success());
    let connection = database();
    let mut state = crate::test_support::app_state(connection);
    state.data_directory = directory.path().to_owned();
    let settings = contracts::CodingSettings {
        enabled: true,
        executable: executable.to_string_lossy().into_owned(),
        ..Default::default()
    };
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE coding_settings SET value_json=?1",
                    [serde_json::to_string(&settings).unwrap()],
                )
                .map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let registered = service::register(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        workspace.to_str().unwrap(),
    )
    .unwrap();
    let workspace_id = registered["workspaceId"].as_str().unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                     VALUES('source',?1,'user','inspect the failures','1')",
                    [crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(crate::database_error)?;
            for (goal, delegation, task) in [
                ("goal-a", "delegation-a", "task-a"),
                ("goal-b", "delegation-b", "task-b"),
            ] {
                connection
                    .execute(
                        "INSERT INTO steward_goals(id,conversation_id,origin,success_condition,status,created_at)
                         VALUES(?1,?2,'user_explicit','test_report_obtained','active','1')",
                        params![goal, crate::PRIMARY_CONVERSATION_ID],
                    )
                    .map_err(crate::database_error)?;
                connection
                    .execute(
                        "INSERT INTO steward_delegations(id,goal_id,conversation_id,workspace_id,ops,budget_runs,budget_ms,notify,status,created_at)
                         VALUES(?1,?2,?3,?4,'read',1,60000,'silent','active','1')",
                        params![delegation, goal, crate::PRIMARY_CONVERSATION_ID, workspace_id],
                    )
                    .map_err(crate::database_error)?;
                connection
                    .execute(
                        "INSERT INTO steward_tasks(id,delegation_id,conversation_id,trigger_kind,source_id,dedupe_key,loop_state,created_at,updated_at)
                         VALUES(?1,?2,?3,'start','source',?4,'queued','1','1')",
                        params![task, delegation, crate::PRIMARY_CONVERSATION_ID, format!("{delegation}:source")],
                    )
                    .map_err(crate::database_error)?;
            }
            Ok(())
        })
        .unwrap();

    let first = service::execute_delegated(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        "task-a",
        workspace_id,
        "wait",
    )
    .unwrap();
    let first_job = first["jobId"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let revision = loop {
        let current = state
            .sqlite_readers
            .read(|connection| {
                repo::inspect(connection, crate::PRIMARY_CONVERSATION_ID, first_job, 0, 1)
            })
            .unwrap();
        if current["delivery"] == "accepted" {
            break current["revision"].as_u64().unwrap();
        }
        assert!(Instant::now() < deadline, "{current}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        service::execute_delegated(
            &state,
            crate::PRIMARY_CONVERSATION_ID,
            "task-b",
            workspace_id,
            "inspect",
        ),
        Err("busy".into())
    );
    let sibling_runs: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM coding_origin_bindings WHERE origin_kind='delegated_event' AND origin_id='task-b'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(sibling_runs, 0);

    state
        .sqlite_writer
        .write(|connection| {
            service::cancel(
                connection,
                crate::PRIMARY_CONVERSATION_ID,
                first_job,
                revision,
                "test slot release",
            )
        })
        .unwrap();
    loop {
        let current = state
            .sqlite_readers
            .read(|connection| {
                repo::inspect(connection, crate::PRIMARY_CONVERSATION_ID, first_job, 0, 1)
            })
            .unwrap();
        if current["state"] == "interrupted" {
            break;
        }
        assert!(Instant::now() < deadline, "{current}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let second = service::execute_delegated(
        &state,
        crate::PRIMARY_CONVERSATION_ID,
        "task-b",
        workspace_id,
        "inspect",
    )
    .unwrap();
    let second_job = second["jobId"].as_str().unwrap();
    loop {
        let current = state
            .sqlite_readers
            .read(|connection| {
                repo::inspect(connection, crate::PRIMARY_CONVERSATION_ID, second_job, 0, 1)
            })
            .unwrap();
        if current["state"] == "settled" {
            break;
        }
        assert!(Instant::now() < deadline, "{current}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
