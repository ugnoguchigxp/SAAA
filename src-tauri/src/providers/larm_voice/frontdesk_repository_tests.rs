use super::{frontdesk_decision::ConversationDecision, frontdesk_repository as repo};
use crate::PRIMARY_CONVERSATION_ID as CONVERSATION;
fn decision(think: bool) -> ConversationDecision {
    ConversationDecision {
        say: "はい。".into(),
        think,
    }
}

fn enable_role_routing(connection: &mut rusqlite::Connection) {
    let mut documents = crate::test_support::default_settings_input();
    let policy = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .unwrap();
    policy.value_json["enabled"] = serde_json::json!(true);
    policy.value_json["actors"] = serde_json::json!([{
        "id":"qwen","label":"Qwen","aliases":[],"transport":"provider",
        "providerId":crate::DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local",
        "resourceGroup":"harness-reasoning","maxInputBytes":65536,"capabilities":["reason"]
    }]);
    policy.value_json["roles"]["reasoner"] = serde_json::json!("qwen");
    policy.value_json["recipes"] = serde_json::json!([{
        "id":"reasoner-response","action":"respond","roles":["reasoner"],"enabled":true
    }]);
    crate::persistence::settings::save_settings_documents_to_connection(connection, &documents)
        .unwrap();
}

#[test]
fn reasoning_request_keeps_all_segments_and_claims_one_qwen_run_without_duplicate_input() {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    enable_role_routing(&mut c);
    repo::accept(&c, CONVERSATION, "u1", "京都へ行きたい。").unwrap();
    assert!(repo::accept(&c, CONVERSATION, "u1", "duplicate").is_err());
    assert!(repo::complete(&c, "u1", &decision(false))
        .unwrap()
        .is_none());
    repo::accept(&c, CONVERSATION, "u2", "2泊3日の計画を作って。").unwrap();
    let (reasoning_request, content) = repo::complete(&c, "u2", &decision(true)).unwrap().unwrap();
    assert_eq!(content, "京都へ行きたい。\n2泊3日の計画を作って。");
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_inputs WHERE root_id IS NULL",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert!(repo::context(&c, CONVERSATION).unwrap().1);
    let state = crate::test_support::app_state(c);
    let mut input: crate::StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_reasoning_request_test","conversationId":CONVERSATION,"content":content,
        "sourceId":reasoning_request,"inputOrigin":"voice","presentationMode":"visual"
    }))
    .unwrap();
    assert_eq!(
        crate::runtime::turns::prepare_runtime_run(&state, &input).unwrap(),
        "conversation"
    );
    state
        .sqlite_readers
        .read(|c| {
            let count: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE content=?1",
                    [&input.content],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
            let scope: String = c
                .query_row(
                    "SELECT status FROM runtime_scope_resolutions WHERE run_id=?1",
                    [&input.run_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(scope, "resolved");
            let adopted: i64 = c
                .query_row(
                    "SELECT count(*) FROM rr_inputs WHERE root_id=?1 AND origin='voice'",
                    [&input.run_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(adopted, 3); // Two ASR segment receipts plus the immutable combined request receipt.
            Ok(())
        })
        .unwrap();
    input.run_id = "run_duplicate".into();
    assert!(crate::runtime::turns::prepare_runtime_run(&state, &input)
        .unwrap_err()
        .contains("already-claimed"));
    state
        .sqlite_writer
        .write(|c| {
            c.execute(
                "UPDATE runtime_runs SET status='completed' WHERE id='run_reasoning_request_test'",
                [],
            )
            .unwrap();
            repo::accept(c, CONVERSATION, "u3", "ありがとう。")?;
            assert!(!repo::context(c, CONVERSATION)?.1);
            let (_, text) = repo::complete(c, "u3", &decision(true))?.unwrap();
            assert_eq!(text, "ありがとう。"); // Already delegated segments cannot silently re-enter a request.
            Ok(())
        })
        .unwrap();
}

#[test]
fn altered_or_cross_conversation_reasoning_request_is_rejected() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    repo::accept(&c, CONVERSATION, "u1", "計算して。").unwrap();
    let (reasoning_request, _) = repo::complete(&c, "u1", &decision(true)).unwrap().unwrap();
    let input: crate::StartTurnInput = serde_json::from_value(serde_json::json!({
        "runId":"run_test","conversationId":CONVERSATION,"content":"違う指示",
        "sourceId":reasoning_request,"inputOrigin":"voice","presentationMode":"visual"
    }))
    .unwrap();
    assert!(repo::claim_reasoning_request(&c, &input).is_err());
}

#[test]
fn legacy_handoff_column_is_migrated_to_reasoning_request() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE conversations(id TEXT PRIMARY KEY);
         CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
         CREATE TABLE lfm_voice_utterances(
           utterance_id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id),
           user_message_id TEXT NOT NULL REFERENCES conversation_messages(id),
           reply_message_id TEXT,
           status TEXT NOT NULL CHECK(status IN ('pending','respond','delegate','failed')),
           handoff_id TEXT UNIQUE,
           request_message_id TEXT,
           claimed_run_id TEXT,
           failure_code TEXT
         );
         INSERT INTO conversations VALUES('c');
         INSERT INTO conversation_messages VALUES('m');
         INSERT INTO lfm_voice_utterances(utterance_id,conversation_id,user_message_id,status,handoff_id)
         VALUES('u','c','m','delegate','lfm_handoff_legacy');",
    ).unwrap();
    repo::migrate(&c).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT reasoning_request_id FROM lfm_voice_utterances WHERE utterance_id='u'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "lfm_handoff_legacy"
    );
}
