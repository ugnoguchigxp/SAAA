use super::*;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
#[test]
fn production_queue_snapshot_prefix_and_manifest_change_fence() {
    crate::steward::with_test_memory(true, || {
        let c = Connection::open_in_memory().unwrap();
        crate::initialize_database(&c).unwrap();
        c.execute("INSERT INTO conversation_messages VALUES('check_memory-fixture',?1,'user','覚えていますか','1')",[PRIMARY_CONVERSATION_ID]).unwrap();
        c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('run_memory-fixture',?1,'conversation.respond','running','check_memory-fixture','1')",[PRIMARY_CONVERSATION_ID]).unwrap();
        let input = StartTurnInput {
            run_id: "run_memory-fixture".into(),
            conversation_id: PRIMARY_CONVERSATION_ID.into(),
            content: "覚えていますか".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let scope = scope::resolve(&c, &input, "check_memory-fixture", true).unwrap();
        let user = scope
            .scopes
            .iter()
            .find(|s| s.kind == "user")
            .unwrap()
            .key
            .clone();
        let body = r#"[{"kind":"preference","key":"食べ物","value":"甘いもの"}]"#;
        let manifest =
            json!({"assertions":[],"inputs":[],"policy":1,"renderer":"memory-snapshot-v1"})
                .to_string();
        c.execute(
            "INSERT INTO personal_snapshots VALUES(?1,'profile',1,?2,?3,?4,1)",
            params![
                user,
                body,
                format!("{:x}", Sha256::digest(body.as_bytes())),
                manifest
            ],
        )
        .unwrap();
        let state = crate::test_support::app_state(c);
        let context =
            compose_for_mode(&state, "memory-fixture", PrefixMode::Stable, false).unwrap();
        assert!(context.history.first().unwrap().body.contains("甘いもの"));
        context.validate_result(&state).unwrap();
        state.sqlite_writer.read_serialized(|c|{context.validate_commit(c)?;c.execute("UPDATE personal_snapshots SET revision=2,manifest=json_set(manifest,'$.renderer','memory-snapshot-v2')",[]).map_err(database_error)?;assert!(context.validate_commit(c).is_err());Ok(())}).unwrap();
        assert!(context.validate_result(&state).is_err());
    });
}
