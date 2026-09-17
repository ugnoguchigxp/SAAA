use super::*;
use rusqlite::{params, Connection};
use serde_json::json;
fn database() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute("UPDATE ui_settings SET enabled=1", []).unwrap();
    c
}
fn input(base: Option<String>, mode: &str) -> contracts::PresentInput {
    contracts::PresentInput { definition:"root = Grid([Cell(models,8),Cell(metric,4)]); models = ModelStatus(\"larm.status\"); metric = Metric(\"runtime.summary\",\"running\",\"実行中\")".into(),summary:"実行状態".into(),mode:mode.into(),base_instance_id:base }
}
#[test]
fn generative_ui_parser_rejects_executable_or_unbounded_definitions() {
    assert!(parser::parse(&input(None, "live").definition).is_ok());
    for invalid in [
        "root=Query(\"anything\")",
        "root=Html(\"<script/>\")",
        "root=Grid([root])",
        "root=Cell(Text(\"x\"),13)",
        "root=ModelStatus(\"https://example.com\")",
        "root=Metric(\"runtime.summary\",\"invented\",\"x\")",
        "root=Text(\"x\"); hidden=Mutation(\"delete\")",
        "root=Text(\"partial",
        "root=Text(\"a\"); root=Text(\"b\")",
    ] {
        assert!(parser::parse(invalid).is_err(), "{invalid}");
    }
}
#[test]
fn generative_ui_revisions_and_snapshots_are_immutable_and_instances_independent() {
    let mut c = database();
    let conversation = crate::PRIMARY_CONVERSATION_ID;
    let tx = c.transaction().unwrap();
    let first = store::create(&tx, conversation, input(None, "snapshot")).unwrap();
    tx.commit().unwrap();
    let id = first["instanceId"].as_str().unwrap();
    let old = store::load(&c, id).unwrap();
    assert!(old.snapshots["runtime.summary"]["rows"][0]["running"].is_number());
    let tx = c.transaction().unwrap();
    let second = store::create(&tx, conversation, input(Some(id.into()), "live")).unwrap();
    tx.commit().unwrap();
    assert_eq!(second["revision"], 2);
    assert_eq!(store::load(&c, id).unwrap().revision, 1);
    let tx = c.transaction().unwrap();
    assert!(store::create(&tx, conversation, input(Some(id.into()), "live")).is_err());
    drop(tx);
    store::save(
        &c,
        contracts::SaveInput {
            instance_id: second["instanceId"].as_str().unwrap().into(),
            name: "ローカルLLM".into(),
            description: "割り当ての状態".into(),
            tags: vec!["監視".into()],
        },
    )
    .unwrap();
    assert_eq!(store::search(&c, "ローカル").unwrap().len(), 1);
    assert_eq!(store::search(&c, "%'").unwrap().len(), 0);
    let tx = c.transaction().unwrap();
    let opened = store::open(&tx, conversation, first["viewId"].as_str().unwrap()).unwrap();
    tx.commit().unwrap();
    assert_ne!(opened["instanceId"], second["instanceId"]);
    assert_eq!(
        store::load(&c, opened["instanceId"].as_str().unwrap())
            .unwrap()
            .state,
        json!({})
    );
    assert_eq!(store::load(&c, id).unwrap().snapshots, old.snapshots);
}
#[test]
fn generative_ui_transaction_rolls_back_on_failure_and_preserves_old_messages() {
    let mut c = database();
    let tx = c.transaction().unwrap();
    assert!(store::create(&tx, "missing-conversation", input(None, "live")).is_err());
    drop(tx);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM ui_views", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = c.transaction().unwrap();
    store::create(&tx, crate::PRIMARY_CONVERSATION_ID, input(None, "live")).unwrap();
    tx.commit().unwrap();
    let page = crate::persistence::list_message_page_from_connection(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        None,
        30,
    )
    .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(page.messages[0].parts.is_some());
    c.execute(
        "INSERT INTO conversation_messages VALUES('old',?1,'user','old text','0')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    let page = crate::persistence::list_message_page_from_connection(
        &c,
        crate::PRIMARY_CONVERSATION_ID,
        None,
        30,
    )
    .unwrap();
    assert!(page.messages[0].parts.is_none());
}
#[test]
fn generative_ui_state_limits_and_data_scoping() {
    let c = database();
    assert!(commands::validate_state(&json!({"filter":"qwen","page":2})).is_ok());
    assert!(commands::validate_state(&json!({"nested":{"password":"secret"}})).is_err());
    assert!(data::query(&c, crate::PRIMARY_CONVERSATION_ID, "arbitrary").is_err());
    c.execute("INSERT INTO conversations(id,task_mode,created_at,updated_at) VALUES('other','coding','0','0')",[]).unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('r','other','conversation.respond','running','100')",[]).unwrap();
    assert_eq!(
        data::query(&c, crate::PRIMARY_CONVERSATION_ID, "runtime.summary")
            .unwrap()
            .rows[0]["running"],
        0
    );
    assert_eq!(
        data::query(&c, "other", "runtime.summary").unwrap().rows[0]["running"],
        1
    );
    c.execute("UPDATE ui_settings SET enabled=?1", params![false])
        .unwrap();
    assert!(store::require_enabled(&c).is_err());
}

#[test]
fn generative_ui_tool_retries_are_idempotent_and_disabled_mode_exposes_no_tools() {
    let c = database();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('ui_run',?1,'conversation.respond','running','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(c);
    let input = crate::StartTurnInput {
        run_id: "ui_run".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "show UI".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let call=crate::runtime::agent_tools::AgentToolCall {id:"ui_call".into(),name:"present_ui".into(),arguments:json!({"definition":"root=ModelStatus(\"runtime.runs\")","summary":"実行履歴","mode":"live"}).to_string()};
    let first = tools::execute(Some(&state), &input, &call);
    let second = tools::execute(Some(&state), &input, &call);
    assert_eq!(first, second);
    assert!(!first.contains("error"));
    for attempt in 0..2 {
        let invalid = crate::runtime::agent_tools::AgentToolCall {
            id: format!("invalid_{attempt}"),
            name: "present_ui".into(),
            arguments: json!({"definition":{"kind":"Query"},"summary":"bad","mode":"live"})
                .to_string(),
        };
        let result = tools::execute(Some(&state), &input, &invalid);
        assert!(result.contains("generationFailure"));
        assert_eq!(result, tools::execute(Some(&state), &input, &invalid));
    }
    let blocked = crate::runtime::agent_tools::AgentToolCall {
        id: "repair_exhausted".into(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
    };
    assert!(tools::execute(Some(&state), &input, &blocked).contains("repair limit"));

    state
        .sqlite_writer
        .write(|c| {
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM ui_instances", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            c.execute("UPDATE ui_settings SET enabled=0", []).unwrap();
            Ok(())
        })
        .unwrap();
    assert!(tools::execute(Some(&state), &input, &call).contains("disabled"));
    let offered = crate::providers::stream::available_agent_tools(
        Some(crate::ProviderOutputPersistence {
            state: &state,
            session_id: "unused",
        }),
        &input,
        0,
        0,
    );
    assert!(!offered
        .iter()
        .any(|t| t["function"]["name"] == "present_ui"));
}

#[test]
fn generative_ui_catalog_is_bounded() {
    let definitions = tools::definitions();
    assert_eq!(definitions.len(), 5);
    assert_eq!(
        definitions[0]["function"]["parameters"]["properties"]["definition"]["type"],
        "object"
    );
    println!(
        "GENUI_CATALOG={}",
        serde_json::to_string(&definitions).unwrap()
    );
}
#[test]
fn generative_ui_semantic_json_has_the_same_strict_execution_boundary() {
    let valid=json!({"kind":"Grid","children":[{"kind":"Cell","span":8,"children":[{"kind":"Table","args":["runtime.runs","provider,status"]}]}]}).to_string();
    let node = parser::parse(&valid).unwrap();
    assert_eq!(node.children[0].span, 8);
    for invalid in [
        json!({"kind":"Cell","children":[]}),
        json!({"kind":"Text","args":["a"],"css":"display:none"}),
        json!({"kind":"Query","args":["arbitrary"]}),
        json!({"kind":"Metric","args":["runtime.summary","fake","fake"]}),
    ] {
        assert!(parser::parse(&invalid.to_string()).is_err());
    }
}

#[test]
fn generative_ui_cancel_checks_scope_and_deduplicates_audit() {
    let mut c = database();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('cancel_target',?1,'conversation.respond','running','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let tx = c.transaction().unwrap();
    let mut request = input(None, "live");
    request.definition =
        "root=Stack([Table(\"runtime.runs\",\"id,status\"),Actions(\"cancel_run\")])".into();
    let created = store::create(&tx, crate::PRIMARY_CONVERSATION_ID, request).unwrap();
    tx.commit().unwrap();
    let id = created["instanceId"].as_str().unwrap().to_string();
    let state = crate::test_support::app_state(c);
    let cancellation = std::sync::Arc::new(crate::RunCancellation::default());
    state
        .active_runs
        .lock()
        .unwrap()
        .insert("cancel_target".into(), cancellation.clone());
    assert!(commands::cancel_for_state(
        &state,
        id.clone(),
        "other_run".into(),
        "request_wrong".into()
    )
    .is_err());
    let first = commands::cancel_for_state(
        &state,
        id.clone(),
        "cancel_target".into(),
        "request_once".into(),
    )
    .unwrap();
    assert!(cancellation.is_cancelled());
    assert_eq!(
        first,
        commands::cancel_for_state(
            &state,
            id.clone(),
            "cancel_target".into(),
            "request_once".into()
        )
        .unwrap()
    );
    assert!(
        commands::cancel_for_state(&state, id, "other_run".into(), "request_once".into()).is_err()
    );
    state
        .sqlite_readers
        .read(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT COUNT(*) FROM audit_events WHERE event_name='ui-cancel-run'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn generative_ui_archive_and_message_cleanup_preserve_published_history() {
    let mut c = database();
    let tx = c.transaction().unwrap();
    let first =
        store::create(&tx, crate::PRIMARY_CONVERSATION_ID, input(None, "snapshot")).unwrap();
    let ephemeral =
        store::create(&tx, crate::PRIMARY_CONVERSATION_ID, input(None, "live")).unwrap();
    tx.commit().unwrap();
    let id = first["instanceId"].as_str().unwrap();
    let view = first["viewId"].as_str().unwrap();
    store::save(
        &c,
        contracts::SaveInput {
            instance_id: id.into(),
            name: "保持".into(),
            description: String::new(),
            tags: vec![],
        },
    )
    .unwrap();
    store::archive(&c, view).unwrap();
    assert!(store::search(&c, "").unwrap().is_empty());
    assert!(store::open(&c, crate::PRIMARY_CONVERSATION_ID, view).is_err());
    assert!(store::load(&c, id).is_ok());
    for result in [&first, &ephemeral] {
        c.execute(
            "DELETE FROM conversation_messages WHERE id=?1",
            [result["messageId"].as_str().unwrap()],
        )
        .unwrap();
    }
    assert!(store::load(&c, id).is_err());
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM ui_views", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM ui_instances", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn generative_ui_every_accepted_tree_can_be_reloaded_after_persistence() {
    let mut c = database();
    let mut definition = "Text(\"leaf\")".to_string();
    for _ in 0..7 {
        definition = format!("Stack([{definition}])");
    }
    let mut request = input(None, "live");
    request.definition = format!("root={definition}");
    let tx = c.transaction().unwrap();
    let result = store::create(&tx, crate::PRIMARY_CONVERSATION_ID, request).unwrap();
    tx.commit().unwrap();
    assert!(store::load(&c, result["instanceId"].as_str().unwrap()).is_ok());
}

#[test]
fn generative_ui_json_rejects_grammar_injection_and_ignored_layout_properties() {
    for node in [
        json!({"kind":"Text(\"unexpected\")\nextra = Text", "args":["x"]}),
        json!({"kind":"Stack", "args":["ignored"]}),
        json!({"kind":"Text", "args":["x"], "span":8}),
    ] {
        assert!(parser::parse(&node.to_string()).is_err());
    }
}

#[test]
fn generative_ui_stored_encoding_must_fit_the_reload_limit() {
    let mut c = database();
    let definition = json!({"kind":"Stack","children":(0..80).map(|_| json!({"kind":"Text","args":["x".repeat(780)]})).collect::<Vec<_>>()}).to_string();
    assert!(definition.len() < 65_536);
    let parsed = parser::parse(&definition).unwrap();
    assert!(serde_json::to_string(&parsed).unwrap().len() > 65_536);
    let mut request = input(None, "live");
    request.definition = definition;
    let tx = c.transaction().unwrap();
    assert!(store::create(&tx, crate::PRIMARY_CONVERSATION_ID, request).is_err());
    drop(tx);
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM ui_views", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn generative_ui_actions_bind_their_required_source_and_unicode_state_matches_ui_limit() {
    let node = parser::parse("root=Actions(\"cancel_run\")").unwrap();
    assert_eq!(parser::sources(&node), vec!["runtime.runs"]);
    assert!(commands::validate_state(&json!({"filter":"あ".repeat(1000)})).is_ok());
    assert!(commands::validate_state(&json!({"filter":"あ".repeat(1001)})).is_err());
}
