use super::{process, session_reader};
use serde_json::json;
use std::io::{BufReader, Cursor};
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
