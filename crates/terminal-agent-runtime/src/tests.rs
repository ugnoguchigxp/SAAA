use super::*;
use serde_json::json;
use std::{fs, path::Path};
fn spec(root: &Path) -> Spec {
    Spec {
        job: "job".into(),
        run: "run".into(),
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        workspace: root.into(),
        executable: "/bin/sh".into(),
        helper: "/tmp/helper with 'quotes'".into(),
        cli: "codex".into(),
        model: String::new(),
        prompt: "original request".into(),
        resume: None,
        answer: None,
        deadline_seconds: 5,
        wake_path: None,
    }
}
#[test]
fn concurrent_events_are_complete_and_unique() {
    let root = tempfile::tempdir().unwrap();
    let spec = spec(root.path());
    create(root.path(), &spec).unwrap();
    std::thread::scope(|scope| {
        for thread in 0..4 {
            let spec = &spec;
            let root = root.path();
            scope.spawn(move || {
                for i in 0..30 {
                    append(root, spec, "test", json!({"thread":thread,"i":i})).unwrap();
                }
            });
        }
    });
    let events = fs::read_to_string(root.path().join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Event>(s).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 120);
    assert_eq!(
        events
            .iter()
            .map(|e| &e.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        120
    );
    assert!(events
        .iter()
        .all(|e| e.run == spec.run && e.nonce == spec.nonce));
}
#[test]
fn private_spec_and_events_have_no_group_access() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let spec = spec(root.path());
    create(root.path(), &spec).unwrap();
    append(root.path(), &spec, "test", json!({})).unwrap();
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["spec.json", "events.jsonl"] {
        assert_eq!(
            fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        read(root.path()).unwrap_err(),
        "terminal_directory_not_private"
    );
}
#[test]
fn argv_preserves_exact_session_and_never_uses_last_or_bypass() {
    let root = tempfile::tempdir().unwrap();
    let mut spec = spec(root.path());
    spec.resume = Some("exact-session".into());
    spec.model = "model with spaces".into();
    let cmd = command(root.path(), &spec).unwrap();
    let args = cmd
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.windows(2).any(|s| s == ["resume", "exact-session"]));
    assert!(!args.iter().any(|s| s.contains("bypass") || s == "--last"));
    assert!(args.contains(&"model with spaces".into()));
    assert_eq!(args.last().unwrap(), "-");
    spec.cli = "claude".into();
    let cmd = command(root.path(), &spec).unwrap();
    assert!(!cmd.get_args().any(|s| s == "-"));
    assert!(cmd.get_args().any(|s| s == "mcp__saaa__saaa_permission"));
}
#[test]
fn hook_paths_are_shell_quoted_without_prompt_or_global_settings() {
    let root = tempfile::tempdir().unwrap();
    let spec = spec(root.path());
    create(root.path(), &spec).unwrap();
    configuration(root.path(), &spec).unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("claude-settings.json")).unwrap())
            .unwrap();
    let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.contains("'\\''"));
    assert!(!command.contains(&spec.prompt));
    assert!(root.path().join("mcp.json").is_file());
}
#[test]
fn oversized_events_fail_without_appending_partial_json() {
    let root = tempfile::tempdir().unwrap();
    let spec = spec(root.path());
    create(root.path(), &spec).unwrap();
    assert!(append(
        root.path(),
        &spec,
        "test",
        json!({"text":"x".repeat(70000)})
    )
    .is_err());
    assert!(!root.path().join("events.jsonl").exists());
}
#[test]
fn cancellation_is_durable_and_run_cannot_be_launched_twice() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fake = root.path().join("fake-cli");
    fs::write(&fake,"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"same-thread\"}'\nsleep 30\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let mut spec = spec(root.path());
    spec.executable = fake;
    create(root.path(), &spec).unwrap();
    write_private(&root.path().join("cancel"), b"test").unwrap();
    process::run(root.path(), &spec).unwrap();
    let events = fs::read_to_string(root.path().join("events.jsonl")).unwrap();
    let last: Event = serde_json::from_str(events.lines().last().unwrap()).unwrap();
    assert_eq!(last.kind, "exit");
    assert_eq!(last.data["stopped"], true);
    assert_eq!(
        process::run(root.path(), &spec).unwrap_err(),
        "terminal_delivery_unknown_do_not_resend"
    );
}
#[test]
fn successful_fake_cli_has_structured_result_and_exit() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let fake = root.path().join("fake-cli");
    fs::write(&fake,"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"same-thread\"}' '{\"type\":\"turn.completed\"}'\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let mut spec = spec(root.path());
    spec.executable = fake;
    create(root.path(), &spec).unwrap();
    process::run(root.path(), &spec).unwrap();
    let events = fs::read_to_string(root.path().join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Event>(s).unwrap())
        .collect::<Vec<_>>();
    assert!(events.iter().any(|e| e.data["type"] == "turn.completed"));
    assert_eq!(events.last().unwrap().data["code"], 0);
}

#[test]
fn configured_bin_symlink_keeps_adjacent_node_on_the_process_path() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let target = root.path().join("codex-wrapper");
    std::fs::write(&target, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let linked = bin.join("codex");
    symlink(&target, &linked).unwrap();
    let resolved = crate::resolve_executable(linked.to_str().unwrap(), "codex").unwrap();
    assert_eq!(resolved, linked);
    let command = crate::cli_command(&resolved);
    let path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .unwrap()
        .1
        .unwrap();
    assert_eq!(std::env::split_paths(path).next().unwrap(), bin);
}
