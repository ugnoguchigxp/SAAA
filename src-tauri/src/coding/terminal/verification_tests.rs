use super::*;
use rusqlite::params;
use serde_json::json;
fn state(
    checks: serde_json::Value,
    candidate: serde_json::Value,
) -> (crate::AppState, tempfile::TempDir) {
    let c = tests::fixture();
    let root = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(root.path())
        .status()
        .unwrap()
        .success());
    c.execute(
        "UPDATE coding_jobs SET workspace_path=?1",
        [root.path().to_string_lossy()],
    )
    .unwrap();
    c.execute("UPDATE terminal_runs SET phase='exited',checks_json=?1,result_json=?2,exit_json=?3,candidate_json=?4",params![checks.to_string(),json!({"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn"}).to_string(),json!({"code":0,"stopped":false,"outputError":false}).to_string(),candidate.to_string()]).unwrap();
    (crate::test_support::app_state(c), root)
}
#[test]
fn saved_checks_and_candidate_are_both_required_for_completion() {
    for (checks, candidate, expected) in [
        (
            json!([]),
            json!({"summary":"ready","remainingManualChecks":[]}),
            "awaiting_user",
        ),
        (json!([["/usr/bin/true"]]), json!(null), "awaiting_user"),
        (
            json!([["/usr/bin/true"]]),
            json!({"summary":"ready","remainingManualChecks":["visual check"]}),
            "awaiting_user",
        ),
        (
            json!([["/usr/bin/true"]]),
            json!({"summary":"ready","remainingManualChecks":[]}),
            "completed",
        ),
    ] {
        let (state, _root) = state(checks, candidate);
        verify::finish(&state, "run").unwrap();
        assert_eq!(
            state
                .sqlite_readers
                .read(|c| c
                    .query_row("SELECT state FROM coding_jobs", [], |r| r
                        .get::<_, String>(0))
                    .map_err(crate::database_error))
                .unwrap(),
            expected
        );
    }
}
#[test]
fn failing_host_check_blocks_completion() {
    let (state, _root) = state(
        json!([["/usr/bin/false"]]),
        json!({"summary":"ready","remainingManualChecks":[]}),
    );
    verify::finish(&state, "run").unwrap();
    assert_eq!(
        state
            .sqlite_readers
            .read(|c| c
                .query_row("SELECT state FROM coding_jobs", [], |r| r
                    .get::<_, String>(0))
                .map_err(crate::database_error))
            .unwrap(),
        "awaiting_user"
    );
}
#[test]
fn interrupted_verification_is_not_automatically_reexecuted() {
    let (state, root) = state(
        json!([["/usr/bin/touch", "should-not-exist"]]),
        json!({"summary":"ready","remainingManualChecks":[]}),
    );
    state
        .sqlite_writer
        .write(|c| {
            c.execute("UPDATE terminal_runs SET phase='verifying'", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    verify::finish(&state, "run").unwrap();
    assert!(!root.path().join("should-not-exist").exists());
    assert_eq!(
        state
            .sqlite_readers
            .read(|c| c
                .query_row("SELECT state FROM coding_jobs", [], |r| r
                    .get::<_, String>(0))
                .map_err(crate::database_error))
            .unwrap(),
        "awaiting_user"
    );
}
#[test]
fn cancellation_during_host_check_cannot_publish_completion() {
    let (state, _root) = state(
        json!([["/bin/sleep", "10"]]),
        json!({"summary":"ready","remainingManualChecks":[]}),
    );
    let writer = state.sqlite_writer.clone();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            writer
                .write(|c| {
                    c.execute("UPDATE coding_runs SET state='stopping'", [])
                        .unwrap();
                    Ok(())
                })
                .unwrap();
        });
        verify::finish(&state, "run").unwrap();
    });
    assert_eq!(
        state
            .sqlite_readers
            .read(|c| c
                .query_row("SELECT state FROM coding_jobs", [], |r| r
                    .get::<_, String>(0))
                .map_err(crate::database_error))
            .unwrap(),
        "interrupted"
    );
}
#[test]
fn retry_is_opt_in_and_creates_a_host_event_not_a_human_message() {
    let (state, _root) = state(
        json!([["/usr/bin/false"]]),
        json!({"summary":"ready","remainingManualChecks":[]}),
    );
    state
        .sqlite_writer
        .write(|c| {
            let saved: String = c
                .query_row("SELECT settings_json FROM coding_jobs", [], |r| r.get(0))
                .unwrap();
            let mut settings: crate::coding::contracts::CodingSettings =
                serde_json::from_str(&saved).unwrap();
            settings.terminal_retry_limit = 2;
            c.execute(
                "UPDATE coding_jobs SET settings_json=?1",
                [serde_json::to_string(&settings).unwrap()],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    // Suppress process launch for this ledger test; test the transaction boundary directly.
    state
        .sqlite_writer
        .write(|c| {
            let tx = c.transaction().unwrap();
            tx.execute("UPDATE coding_runs SET state='settled'", [])
                .unwrap();
            tx.execute("UPDATE coding_jobs SET state='awaiting_user'", [])
                .unwrap();
            let next = retry::after_checks(
                &tx,
                "job",
                crate::PRIMARY_CONVERSATION_ID,
                "run",
                &json!({"verification":{"checks":[{"exitCode":1}]}}),
            )
            .unwrap()
            .unwrap();
            assert_ne!(next, "run");
            assert_eq!(
                tx.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert!(crate::coding::repository::authorize(
                &tx,
                "job",
                crate::PRIMARY_CONVERSATION_ID
            )
            .is_ok());
            tx.rollback().unwrap();
            Ok(())
        })
        .unwrap();
}
#[test]
fn forged_process_identity_is_never_signalled() {
    let (state, root) = state(json!([]), json!(null));
    let nonce = "n".repeat(32);
    let spec = saaa_terminal_agent_runtime::Spec {
        job: "job".into(),
        run: "run".into(),
        nonce: nonce.clone(),
        workspace: root.path().into(),
        executable: "/usr/bin/true".into(),
        helper: "/usr/bin/true".into(),
        cli: "claude".into(),
        model: String::new(),
        prompt: String::new(),
        resume: None,
        answer: None,
        deadline_seconds: 5,
        wake_path: None,
    };
    saaa_terminal_agent_runtime::create(root.path(), &spec).unwrap();
    saaa_terminal_agent_runtime::write_private(
        &root.path().join("child.json"),
        &serde_json::to_vec(
            &json!({"run":"run","nonce":nonce,"pid":std::process::id(),"identity":"forged"}),
        )
        .unwrap(),
    )
    .unwrap();
    state
        .sqlite_writer
        .write(|c| {
            c.execute(
                "UPDATE terminal_runs SET directory=?1,nonce=?2,phase='blocked'",
                params![root.path().to_string_lossy(), nonce],
            )
            .unwrap();
            c.execute("UPDATE coding_jobs SET state='outcome_unknown'", [])
                .unwrap();
            c.execute("UPDATE coding_runs SET state='outcome_unknown'", [])
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        recovery::stop_unknown(&state, crate::PRIMARY_CONVERSATION_ID, "job", 1).unwrap_err(),
        "old_process_identity_mismatch_manual_stop_required"
    );
}
