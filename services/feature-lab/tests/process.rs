//! Process-level contracts. flock is per process, so a second open in this process cannot prove it.
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde_json::json;

#[test]
fn sigint_runs_shutdown_and_a_second_process_cannot_rewrite_rows() {
    let directory = std::env::temp_dir().join(format!("saaa-lab-process-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let database = directory.join("lab.sqlite");
    let bin = env!("CARGO_BIN_EXE_saaa-feature-lab");
    let mut owner = spawn_lab(bin, &database);
    let stderr = owner.child.stderr.take().unwrap();
    let mut stdout = BufReader::new(owner.child.stdout.take().unwrap());
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    assert!(ready.contains("\"ready\":true"), "{ready}");

    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "INSERT INTO purpose_media_operations(run_id,kind,route_json,state,remote_id,updated_at) VALUES('run','\"music\"','{}','reserved','job-1','1')",
            [],
        )
        .unwrap();
    drop(connection);

    let mut second = spawn_lab(bin, &database);
    let status = wait_bounded(&mut second, Duration::from_secs(8));
    let mut second_error = String::new();
    second
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut second_error)
        .unwrap();
    assert!(!status.success(), "{second_error}");
    assert!(second_error.contains("already owned"), "{second_error}");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let state: String = connection
        .query_row(
            "SELECT state FROM purpose_media_operations WHERE run_id='run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "reserved");
    drop(connection);

    let _keep_stdin_open = &owner.stdin;
    let _ = Command::new("kill")
        .args(["-INT", &owner.child.id().to_string()])
        .status()
        .unwrap();
    let status = wait_bounded(&mut owner, Duration::from_secs(8));
    assert!(status.success(), "SIGINT must reach shutdown");
    drop(stderr);
    let _ = std::fs::remove_dir_all(directory);
}

struct LabChild {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    reaped: bool,
}

impl Drop for LabChild {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        let _ = Command::new("kill")
            .args(["-KILL", &self.child.id().to_string()])
            .status();
        let _ = self.child.wait();
    }
}

fn spawn_lab(bin: &str, database: &std::path::Path) -> LabChild {
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let config = json!({
        "databasePath": database,
        "allowedOrigin": "http://127.0.0.1:1422",
        "sessionToken": "0123456789abcdef0123456789abcdef",
        "provider": "fixture",
        "larmEndpoint": "http://127.0.0.1:9/",
        "larmToken": null
    });
    writeln!(stdin, "{config}").unwrap();
    stdin.flush().unwrap();
    LabChild {
        child,
        stdin,
        reaped: false,
    }
}

fn wait_bounded(lab: &mut LabChild, limit: Duration) -> std::process::ExitStatus {
    let pid = lab.child.id();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if cancel_rx.recv_timeout(limit).is_err() {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    });
    let status = lab.child.wait().unwrap();
    lab.reaped = true;
    let _ = cancel_tx.send(());
    status
}
