use super::*;
#[test]
pub(super) fn rr_04_queue_full_leaves_no_input_message() {
    let mut c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
    let mut policy = RoleRoutingSettings {
        enabled: true,
        ..Default::default()
    };
    policy.limits.max_queued_inputs = 1;
    policy.actors.push(RoutingActor {
        id: "qwen".into(),
        label: "Qwen".into(),
        aliases: vec![],
        transport: "provider".into(),
        provider_id: Some("qwen".into()),
        model: None,
        location: "local".into(),
        resource_group: "gpu".into(),
        max_input_bytes: 1024,
        larm_provider: None,
        capabilities: vec!["reason".into()],
    });
    policy.roles.reasoner = Some("qwen".into());
    policy.recipes.push(RoutingRecipe {
        id: "respond".into(),
        action: crate::role_routing::contracts::RoutingAction::Respond,
        roles: vec!["reasoner".into()],
        enabled: true,
    });
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,?1,'d',1)",
        [serde_json::to_string(&policy).expect("policy")],
    )
    .expect("policy row");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('queued','c','p','queued','text','visual',1,'')", []).expect("queued root");
    {
        let tx = c.transaction().expect("tx");
        tx.execute(
            "INSERT INTO conversation_messages VALUES('u','c','user','hello','1')",
            [],
        )
        .expect("message");
        tx.execute(
            "INSERT INTO runtime_runs VALUES('run','c','conversation.respond','u')",
            [],
        )
        .expect("run");
        assert_eq!(
            record_provider_turn_start_in_transaction(
                &tx,
                "run",
                "c",
                "text",
                None,
                "visual",
                2,
                &crate::role_routing::selection::SelectionInput {
                    cloud_allowed: true,
                    ..Default::default()
                },
            ),
            Err("Role-routing input queue is full".into())
        );
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
            .get::<_, i64>(0))
            .expect("messages"),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_roots", [], |r| r.get::<_, i64>(0))
            .expect("roots"),
        1
    );
}
#[test]
pub(super) fn ai_08_provider_terminal_result_is_recorded_for_the_dispatch_decision() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('u','c','user','hello','1');INSERT INTO runtime_runs VALUES('run','c','conversation.respond','u');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
    let mut policy = RoleRoutingSettings {
        enabled: true,
        ..Default::default()
    };
    policy.adaptive_improvement.enabled = true;
    policy.adaptive_improvement.provider_recipe = true;
    policy.actors.push(RoutingActor {
        id: "qwen".into(),
        label: "Qwen".into(),
        aliases: vec![],
        transport: "provider".into(),
        provider_id: Some("qwen".into()),
        model: None,
        location: "local".into(),
        resource_group: "gpu".into(),
        max_input_bytes: 1024,
        larm_provider: None,
        capabilities: vec!["reason".into()],
    });
    policy.roles.reasoner = Some("qwen".into());
    policy.recipes.push(RoutingRecipe {
        id: "respond".into(),
        action: crate::role_routing::contracts::RoutingAction::Respond,
        roles: vec!["reasoner".into()],
        enabled: true,
    });
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,?1,'d',1)",
        [serde_json::to_string(&policy).expect("policy")],
    )
    .expect("policy row");
    assert!(record_provider_turn_start(&c, "run", "c", 1).expect("start"));
    c.execute(
        "INSERT INTO conversation_messages VALUES('a','c','assistant','answer','2')",
        [],
    )
    .expect("answer");
    record_provider_turn_finish(&c, "run", "completed", Some("a"), 2).expect("finish");
    let outcome: i64 = c
        .query_row(
            "SELECT technical_success FROM ai_outcomes WHERE decision_id='ai-provider-run'",
            [],
            |row| row.get(0),
        )
        .expect("adaptive outcome");
    assert_eq!(outcome, 1);
}
#[test]
pub(super) fn rr_12_acceptance_commits_message_and_root_together() {
    let mut c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p','responding','text','visual',1,'')",[]).expect("root");
    c.execute("INSERT INTO rr_decisions VALUES('d','run',0,NULL,'{}','[]','recipe','respond','[]','rules-v1','p',1)",[]).expect("decision");
    c.execute("INSERT INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('rr-step-run-0','run','d',0,0,'qwen','respond','running','{}','{}')",[]).expect("step");
    c.execute(
        "INSERT INTO rr_events VALUES('run',1,'input_accepted','{}',1)",
        [],
    )
    .expect("event");
    c.execute(
        "INSERT INTO rr_events VALUES('run',2,'root_started','{}',1)",
        [],
    )
    .expect("start event");
    let tx = c.transaction().expect("tx");
    tx.execute(
        "INSERT INTO conversation_messages VALUES('a','c','assistant','answer','2')",
        [],
    )
    .expect("message");
    accept_provider_turn(&tx, "run", "a", 2).expect("accept");
    tx.commit().expect("commit");
    assert_eq!(
        c.query_row(
            "SELECT result_message_id FROM rr_roots WHERE root_id='run'",
            [],
            |r| r.get::<_, String>(0)
        )
        .expect("root"),
        "a"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_outputs", [], |r| r
            .get::<_, i64>(0))
            .expect("output"),
        1
    );
    let event: (i64, String) = c
        .query_row(
            "SELECT seq,kind FROM rr_events WHERE root_id='run' ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("terminal event");
    assert_eq!(event, (3, "answer_committed".into()));
    record_provider_turn_finish(&c, "run", "completed", Some("a"), 3)
        .expect("outer terminal is idempotent");
    let event_count: i64 = c
        .query_row(
            "SELECT count(*) FROM rr_events WHERE root_id='run'",
            [],
            |row| row.get(0),
        )
        .expect("event count");
    assert_eq!(event_count, 3);
}
#[test]
pub(super) fn rr_12_old_revision_result_rejected() {
    let mut c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    // The root has advanced to revision 1, but the running result belongs to revision 0.
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p',1,'responding','text','visual',1,'')",[]).expect("root");
    c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('rr-step-run-0','run',0,0,'qwen','respond','running','{}','{}')",[]).expect("step");
    {
        let tx = c.transaction().expect("tx");
        tx.execute(
            "INSERT INTO conversation_messages VALUES('a','c','assistant','answer','2')",
            [],
        )
        .expect("message");
        assert!(
            accept_provider_turn(&tx, "run", "a", 2).is_err(),
            "a revision-0 result must not be adopted against revision 1"
        );
        // The caller rolls the assistant message back with the rejected adoption.
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
            .get::<_, i64>(0))
            .expect("messages"),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_outputs", [], |r| r
            .get::<_, i64>(0))
            .expect("outputs"),
        0
    );
    let root: (i64, String) = c
        .query_row(
            "SELECT revision,phase FROM rr_roots WHERE root_id='run'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("root");
    assert_eq!(root, (1, "responding".into()));
}
#[test]
pub(super) fn rr_16_pending_input_blocks_real_acceptance() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    // `draining` is the durable input-barrier phase. A provider result arriving here is held.
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p',0,'draining','text','visual',1,'')",[]).expect("root");
    c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('rr-step-run-0','run',0,0,'qwen','respond','draining','{}','{}')",[]).expect("step");
    c.execute(
        "INSERT INTO rr_events VALUES('run',1,'input_accepted','{}',1)",
        [],
    )
    .expect("event");
    c.execute(
        "INSERT INTO rr_events VALUES('run',2,'input_barrier','{}',1)",
        [],
    )
    .expect("barrier event");
    assert!(
        accept_provider_turn(&c, "run", "a", 2).is_err(),
        "a held result must not be adopted while the barrier is up"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_outputs", [], |r| r
            .get::<_, i64>(0))
            .expect("outputs"),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_events WHERE root_id='run' AND kind='answer_committed'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .expect("committed events"),
        0
    );
    let phase: String = c
        .query_row(
            "SELECT phase FROM rr_roots WHERE root_id='run'",
            [],
            |row| row.get(0),
        )
        .expect("root phase");
    assert_eq!(phase, "draining");
}
#[test]
pub(super) fn rr_12_db_failure_no_speech() {
    let mut c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p',0,'responding','text','visual',1,'')",[]).expect("root");
    c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('rr-step-run-0','run',0,0,'qwen','respond','running','{}','{}')",[]).expect("step");
    // Force the output insert to fail so the adoption transaction must roll back.
    c.execute("INSERT INTO rr_outputs VALUES('rr-output-rr-step-run-0','rr-step-run-0',0,'answer','{}',1,1)",[]).expect("collision output");
    {
        let tx = c.transaction().expect("tx");
        tx.execute(
            "INSERT INTO conversation_messages VALUES('a','c','assistant','answer','2')",
            [],
        )
        .expect("message");
        assert!(
            accept_provider_turn(&tx, "run", "a", 2).is_err(),
            "a DB failure must surface instead of silently succeeding"
        );
    }
    // No committed answer means no accepted output and therefore no speech intent.
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_events WHERE root_id='run' AND kind='answer_committed'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .expect("committed events"),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
            .get::<_, i64>(0))
            .expect("messages"),
        0
    );
    let phase: String = c
        .query_row(
            "SELECT phase FROM rr_roots WHERE root_id='run'",
            [],
            |row| row.get(0),
        )
        .expect("root phase");
    assert_eq!(phase, "responding");
}
#[test]
pub(super) fn rr_04_receipt_retry_and_conflict() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('m1');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    assert_eq!(
        record_input_receipt(
            &c, None, "in-1", "c", "m1", "digest-a", None, "text", "accepted", 0, 1
        )
        .expect("first receipt"),
        InputReceiptDisposition::Accepted
    );
    // The same input and payload returns the original receipt without a second row.
    assert_eq!(
        record_input_receipt(
            &c, None, "in-1", "c", "m1", "digest-a", None, "text", "accepted", 0, 2
        )
        .expect("retry"),
        InputReceiptDisposition::Duplicate
    );
    assert!(
        record_input_receipt(
            &c, None, "in-1", "c", "m1", "digest-b", None, "text", "accepted", 0, 3
        )
        .is_err(),
        "the same input id with a changed payload is a conflict"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_inputs", [], |row| row
            .get::<_, i64>(0))
            .expect("count"),
        1
    );
}
#[test]
pub(super) fn rr_14_asr_duplicate_receipt() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('m1');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    assert_eq!(
        record_input_receipt(
            &c,
            None,
            "in-1",
            "c",
            "m1",
            "digest-a",
            Some("asr-src-1"),
            "voice",
            "accepted",
            0,
            1
        )
        .expect("first"),
        InputReceiptDisposition::Accepted
    );
    // A repeated ASR final shares the source id but arrives with a new input id.
    assert_eq!(
        record_input_receipt(
            &c,
            None,
            "in-2",
            "c",
            "m1",
            "digest-a",
            Some("asr-src-1"),
            "voice",
            "accepted",
            0,
            2
        )
        .expect("retransmit"),
        InputReceiptDisposition::Duplicate
    );
    let conflict = record_input_receipt(
        &c,
        None,
        "in-3",
        "c",
        "m1",
        "digest-b",
        Some("asr-src-1"),
        "voice",
        "accepted",
        0,
        3,
    )
    .expect_err("same source with a different payload conflicts");
    assert!(conflict.contains("source conflicts"));
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_inputs", [], |row| row
            .get::<_, i64>(0))
            .expect("count"),
        1
    );
}
#[test]
pub(super) fn rr_16_multiple_pending_inputs() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('m1');INSERT INTO conversation_messages VALUES('m2');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p',0,'responding','text','visual',1,'')", []).expect("root");
    assert_eq!(
        record_active_input_barrier(&c, "r", "in-1", "c", "m1", "d1", None, "text", 2)
            .expect("first barrier"),
        InputReceiptDisposition::Accepted
    );
    assert_eq!(
        record_active_input_barrier(&c, "r", "in-2", "c", "m2", "d2", None, "text", 3)
            .expect("second barrier"),
        InputReceiptDisposition::Accepted
    );
    // Neither pending input is dropped and the stored generation is the barrier revision.
    assert_eq!(pending_input_count(&c, "r").expect("pending"), 2);
    assert!(classifier_generation_matches(&c, "in-1", "c", 0).expect("generation"));
    assert!(!classifier_generation_matches(&c, "in-1", "c", 1).expect("stale generation"));
    let phase: String = c
        .query_row("SELECT phase FROM rr_roots WHERE root_id='r'", [], |row| {
            row.get(0)
        })
        .expect("phase");
    assert_eq!(phase, "draining");
}
#[test]
pub(super) fn rr_22_usage_is_saved_with_the_active_step() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p','responding','text','visual',1,'')", [])
        .expect("root");
    c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('rr-step-run-0','run',0,0,'sol','respond','running','{}','{}')", [])
        .expect("step");
    record_step_usage(
        &c,
        "run",
        r#"{"inputTokens":8,"cachedInputTokens":3,"outputTokens":5,"reasoningOutputTokens":2}"#,
    )
    .expect("usage");
    assert_eq!(
        c.query_row(
            "SELECT usage_json FROM rr_steps WHERE id='rr-step-run-0'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("usage row"),
        r#"{"inputTokens":8,"cachedInputTokens":3,"outputTokens":5,"reasoningOutputTokens":2}"#
    );
}
