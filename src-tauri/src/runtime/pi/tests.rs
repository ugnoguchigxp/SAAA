use super::{process, session_reader};
use serde_json::json;
use std::io::{BufReader, Cursor};

#[cfg(target_os = "macos")]
#[test]
fn delegated_profile_allows_only_the_kernel_read_needed_to_start_pi() {
    let settings = crate::coding::contracts::CodingSettings {
        profile: "delegated-read-test-macos-v1".into(),
        ..Default::default()
    };
    let workspace = tempfile::tempdir().unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let session = session_dir.path().join("session.jsonl");
    let command = process::delegated_command(&settings, workspace.path(), &session).unwrap();
    let args = format!("{command:?}");
    assert!(args.contains("allow sysctl-read"));
    assert!(args.contains("settings.json.lock"));
    assert!(args.contains("auth.json.lock"));
    assert!(args.contains("delegated-sdk-state"));
    assert!(args.contains("deny default"));
    assert!(!args.contains("allow default"));
    assert!(!args.contains("network"));
}
#[test]
fn pi_records_preserve_utf8_and_unicode_newlines_and_reject_truncation() {
    let bytes = b"{\"text\":\"\xe6\x97\xa5\xe2\x80\xa8x\"}\n";
    let mut reader = BufReader::with_capacity(1, Cursor::new(bytes));
    assert_eq!(
        process::record(&mut reader).unwrap().unwrap()["text"],
        "日\u{2028}x"
    );
    assert!(process::record(&mut reader).unwrap().is_none());
    for bytes in [b"{\"partial\":".as_slice(), b"[]\n", b"{\"x\":\"\xff\"}\n"] {
        assert!(process::record(&mut Cursor::new(bytes)).is_err());
    }
    assert!(process::record(&mut Cursor::new(vec![b'x'; process::LINE_LIMIT + 1])).is_err());
}
#[test]
fn pi_session_reader_scopes_results_to_boundary_and_rejects_missing_or_corrupt_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let cwd = dir.path();
    let records = [
        json!({"type":"session","id":"s","cwd":cwd}),
        json!({"id":"a","parentId":null,"message":{"role":"assistant","content":[{"type":"text","text":"old"}]}}),
        json!({"id":"b","parentId":"a","message":{"role":"toolResult","toolName":"bash","isError":true}}),
        json!({"type":"custom","customType":"saaa.codex-sdk.observation","id":"sdk","parentId":"b","data":{"kind":"file_change","isError":true}}),
        json!({"id":"c","parentId":"sdk","message":{"role":"assistant","stopReason":"error","content":[{"type":"text","text":"new"}]}}),
    ];
    std::fs::write(
        &path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let result = session_reader::read(&path, cwd, Some("a")).unwrap();
    assert_eq!(result.summary, "new");
    assert_eq!(result.errors, 3);
    assert_eq!(result.tools.len(), 2);
    assert_eq!(result.tools[1]["tool"], "file_change");
    assert_eq!(result.leaf.as_deref(), Some("c"));
    assert!(session_reader::read(&path, cwd, Some("missing")).is_err());
    assert!(session_reader::read(&path, std::path::Path::new("/wrong"), None).is_err());
    std::fs::write(&path, "{\"type\":").unwrap();
    assert!(session_reader::read(&path, cwd, None).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(session_reader::read(&path, cwd, None).is_err());
}

#[test]
#[ignore = "explicit local pi probe; set SAAA_PI_BINARY, no model prompt is sent"]
fn pi_installed_binary_probe_reports_capability_without_sending_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let settings = crate::coding::contracts::CodingSettings {
        executable: std::env::var("SAAA_PI_BINARY").expect("SAAA_PI_BINARY"),
        ..Default::default()
    };
    let result = process::probe(&settings, directory.path());
    eprintln!("pi capability probe: {result:?}");
    assert!(
        result.is_ok()
            || matches!(
                result.as_ref().err().map(String::as_str),
                Some("authentication_required" | "model_unavailable")
            ),
        "{result:?}"
    );
}

#[test]
#[ignore = "explicit local SDK probe; set SAAA_PI_BINARY and SAAA_PI_SDK_EXTENSION, no model prompt is sent"]
fn delegated_sdk_profile_probe_uses_isolated_writable_state() {
    let directory = tempfile::tempdir().unwrap();
    let settings = crate::coding::contracts::CodingSettings {
        executable: std::env::var("SAAA_PI_BINARY").expect("SAAA_PI_BINARY"),
        profile: "delegated-codex-sdk-macos-v1".into(),
        provider: "saaa-codex-sdk".into(),
        sdk_extension_path: Some(
            std::env::var("SAAA_PI_SDK_EXTENSION").expect("SAAA_PI_SDK_EXTENSION"),
        ),
        ..Default::default()
    };
    let result = process::probe(&settings, directory.path());
    eprintln!("delegated SDK capability probe: {result:?}");
    assert!(
        result.is_ok()
            || matches!(
                result.as_ref().err().map(String::as_str),
                Some("authentication_required" | "model_unavailable")
            ),
        "{result:?}"
    );
}

#[test]
#[ignore = "explicit authenticated SDK read-only acceptance; set SAAA_PI_BINARY and SAAA_PI_SDK_EXTENSION"]
fn delegated_sdk_profile_completes_a_read_only_prompt() {
    let workspace = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(workspace.path())
        .status()
        .unwrap()
        .success());
    std::fs::write(workspace.path().join("README.md"), "read-only fixture\n").unwrap();
    let settings = crate::coding::contracts::CodingSettings {
        executable: std::env::var("SAAA_PI_BINARY").expect("SAAA_PI_BINARY"),
        profile: "delegated-codex-sdk-macos-v1".into(),
        provider: "saaa-codex-sdk".into(),
        sdk_extension_path: Some(
            std::env::var("SAAA_PI_SDK_EXTENSION").expect("SAAA_PI_SDK_EXTENSION"),
        ),
        ..Default::default()
    };
    let session = workspace.path().join("session.jsonl");
    let mut process = process::Process::open(&settings, workspace.path(), &session).unwrap();
    process::ready(&mut process, &settings, &session).unwrap();
    let prompt = process
        .send(
            "prompt",
            json!({"message":"Read README.md and briefly report its content. Do not edit files or run commands."}),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let mut accepted = false;
    let mut settled = false;
    while std::time::Instant::now() < deadline && !settled {
        if let Some(event) = process.next(std::time::Duration::from_millis(100)).unwrap() {
            accepted |=
                event["type"] == "response" && event["id"] == prompt && event["success"] == true;
            settled |= event["type"] == "agent_settled";
        }
    }
    let closed = process.close();
    assert!(
        accepted && settled,
        "SDK prompt was not accepted and settled"
    );
    assert!(closed.is_ok(), "{closed:?}");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("README.md")).unwrap(),
        "read-only fixture\n"
    );
}
