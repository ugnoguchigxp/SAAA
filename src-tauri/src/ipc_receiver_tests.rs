#![cfg(test)]
use super::*;

#[test]
fn ipc_receiver_fixture_matches_rust_serialization() {
    let connection = Connection::open_in_memory().unwrap();
    initialize_database(&connection).unwrap();
    let state = test_support::app_state(connection);
    let snapshot = persistence::app_commands::get_app_snapshot(&state).unwrap();
    let runtime = ipc_contract::RuntimeEvent::Started {
        run_id: "run-fixture".into(),
        route: "conversation.respond".into(),
        provider_id: "provider-fixture".into(),
    };
    let meeting = meeting::MeetingEvent::TranscriptFinal {
        session_id: "meeting-fixture".into(),
        lane: meeting::MeetingLane::Microphone,
        sequence: 1,
        text: "Fixture transcript".into(),
        language: Some("ja".into()),
    };
    let asr = voice::streaming_asr::contracts::VoiceAsrStreamEvent::Final {
        session_id: "asr-fixture".into(),
        utterance_id: "utterance-fixture".into(),
        revision: 1,
        start_ms: 0,
        end_ms: 1000,
        text: "Fixture transcript".into(),
        language: Some("ja".into()),
    };
    let mut value = serde_json::json!({ "snapshot": snapshot, "runtime": runtime, "meeting": meeting, "asr": asr });
    fn stable_timestamps(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    if matches!(key.as_str(), "updatedAt" | "createdAt") && value.is_string() {
                        *value = serde_json::json!("0");
                    } else {
                        stable_timestamps(value);
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    stable_timestamps(value);
                }
            }
            _ => (),
        }
    }
    stable_timestamps(&mut value);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/ipc-receivers.json");
    if std::env::var_os("SAAA_UPDATE_IPC_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
    }
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path)
            .expect("generate receiver fixtures with bun run ipc:fixtures"),
    )
    .unwrap();
    assert_eq!(
        value, expected,
        "Rust IPC fixture changed; regenerate and verify frontend schemas"
    );
}
