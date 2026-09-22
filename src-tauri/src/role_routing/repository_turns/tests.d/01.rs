fn review_flow_fixture() -> Connection {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c'); INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning");
    let mut policy = RoleRoutingSettings::default();
    policy.limits.max_review_rounds = 1;
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,?1,'d',1)",
        [serde_json::to_string(&policy).expect("policy")],
    )
    .expect("policy row");
    c.execute_batch("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p',0,'responding','reasoning','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms,completed_at_ms) VALUES('draft','run',0,0,'author','respond','succeeded','a','{}',1,2),('review','run',0,1,'reviewer','review','running','b','{}',2,NULL),('revise','run',0,2,'author','revise','planned','c','{}',NULL,NULL); INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES('rr-output-draft','draft',0,'intermediate','{\"sha256\":\"d\",\"bytes\":5,\"hostVerification\":\"verified\",\"verifierVersion\":\"fixture-v1\"}',0,2); INSERT INTO rr_events VALUES('run',1,'input_accepted','{}',1);").expect("review plan");
    c
}
fn final_flow_fixture() -> Connection {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c'); INSERT INTO runtime_runs VALUES('run');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning");
    c.execute_batch("INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1); INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','run','p',0,'responding','reasoning','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('step','run',0,0,'author','respond','running','f','{}',1);").expect("routing fixture");
    c
}
#[test]
fn rr_29_cancel_completion_both_orders() {
    let mut cancel_first = final_flow_fixture();
    crate::role_routing::coordinator::apply(
        &mut cancel_first,
        "run",
        crate::role_routing::reducer::Event::Cancel,
        2,
    )
    .expect("cancel commits first");
    {
        let transaction = cancel_first.transaction().expect("transaction");
        transaction
            .execute(
                "INSERT INTO conversation_messages VALUES('late','c','assistant','late answer','3')",
                [],
            )
            .expect("tentative answer");
        assert!(accept_provider_turn(&transaction, "run", "late", 3).is_err());
    }
    assert_eq!(
        cancel_first
            .query_row(
                "SELECT phase||':'||(SELECT count(*) FROM conversation_messages)
                 FROM rr_roots WHERE root_id='run'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("cancel-first state"),
        "cancelled:0"
    );

    let mut completion_first = final_flow_fixture();
    {
        let transaction = completion_first.transaction().expect("transaction");
        transaction
            .execute(
                "INSERT INTO conversation_messages VALUES('answer','c','assistant','accepted answer','2')",
                [],
            )
            .expect("answer");
        accept_provider_turn(&transaction, "run", "answer", 2).expect("completion wins");
        transaction.commit().expect("completion commits");
    }
    crate::role_routing::coordinator::apply(
        &mut completion_first,
        "run",
        crate::role_routing::reducer::Event::Cancel,
        3,
    )
    .expect("late cancel is idempotent");
    assert_eq!(
        completion_first
            .query_row(
                "SELECT phase||':'||result_message_id||':'||
                   (SELECT count(*) FROM rr_outputs WHERE accepted=1)
                 FROM rr_roots WHERE root_id='run'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("completion-first state"),
        "completed:answer:1"
    );
}
fn review_issue(verdict: &str) -> crate::role_routing::review::ReviewIssue {
    crate::role_routing::review::ReviewIssue {
        kind: "logic".into(),
        claim: "the conclusion does not follow".into(),
        severity: "major".into(),
        code: "non-sequitur".into(),
        evidence_ref: "rr-output-draft".into(),
        verdict: verdict.into(),
    }
}
#[test]
fn rr_25_decision_consumed_once_and_claims_revise_atomically() {
    let c = review_flow_fixture();
    let response = crate::role_routing::review::ReviewResponse {
        issues: vec![review_issue("verified")],
    };
    let outcome = advance_review_step(&c, "run", &response, None, 3).expect("review");
    assert!(matches!(outcome, ReviewStepOutcome::Revise(_)));
    let statuses = c
        .prepare("SELECT status FROM rr_steps WHERE root_id='run' ORDER BY ordinal")
        .expect("query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("statuses");
    assert_eq!(statuses, vec!["succeeded", "succeeded", "running"]);
    assert!(advance_review_step(&c, "run", &response, None, 4).is_err());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_outputs WHERE kind='revision-decision'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .expect("decision count"),
        1
    );
}
#[test]
fn rr_25_no_verified_issue_keeps_the_reviewed_draft() {
    let mut c = review_flow_fixture();
    let response = crate::role_routing::review::ReviewResponse { issues: vec![] };
    assert!(matches!(
        advance_review_step(&c, "run", &response, None, 3).expect("review"),
        ReviewStepOutcome::KeepDraft
    ));
    c.execute(
        "INSERT INTO conversation_messages VALUES('answer','c','assistant','draft','4')",
        [],
    )
    .expect("answer");
    let tx = c.transaction().expect("tx");
    accept_reviewed_draft(&tx, "run", "draft", "answer", 4).expect("accept draft");
    tx.commit().expect("commit");
    assert_eq!(
        c.query_row(
            "SELECT phase FROM rr_roots WHERE root_id='run'",
            [],
            |row| row.get::<_, String>(0)
        )
        .expect("phase"),
        "completed"
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_outputs WHERE kind='answer' AND accepted=1",
            [],
            |row| row.get::<_, i64>(0)
        )
        .expect("answer output"),
        1
    );
}
#[test]
fn rr_26_unresolved_review_proposes_and_consumes_premium_once() {
    let c = review_flow_fixture();
    let mut policy: RoleRoutingSettings = serde_json::from_str(
        &c.query_row(
            "SELECT config_json FROM rr_policy_versions WHERE id='p'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("policy"),
    )
    .expect("valid policy");
    policy.actors.push(RoutingActor {
        id: "astra".into(),
        label: "Astra".into(),
        aliases: vec![],
        transport: "codex_sdk".into(),
        provider_id: None,
        model: Some("gpt-6-astra".into()),
        location: "cloud".into(),
        resource_group: "cloud".into(),
        max_input_bytes: 4096,
        capabilities: vec!["reason".into()],
    });
    policy.roles.premium = Some("astra".into());
    c.execute(
        "UPDATE rr_policy_versions SET config_json=?1 WHERE id='p'",
        [serde_json::to_string(&policy).expect("policy json")],
    )
    .expect("update policy");
    let response = crate::role_routing::review::ReviewResponse {
        issues: vec![review_issue("unresolved")],
    };
    let receipt = match advance_review_step(&c, "run", &response, None, 3).expect("review") {
        ReviewStepOutcome::AwaitPremium(receipt) => receipt,
        _ => panic!("unresolved review should await explicit premium approval"),
    };
    assert_eq!(receipt.candidate_id, "astra");
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM rr_steps WHERE actor_id='astra'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .expect("premium starts"),
        0,
        "a proposal alone must never create or start a premium step"
    );
    crate::role_routing::proposals::approve(
        &c,
        &crate::role_routing::proposals::Approval {
            proposal_id: receipt.id.clone(),
            candidate_id: receipt.candidate_id.clone(),
        },
        4,
        true,
    )
    .expect("approve");
    let consumed = crate::role_routing::proposals::consume_approval(
        &c,
        &crate::role_routing::proposals::Approval {
            proposal_id: receipt.id.clone(),
            candidate_id: receipt.candidate_id.clone(),
        },
        "p",
        0,
        true,
        5,
    )
    .expect("consume");
    assert_eq!(
        crate::role_routing::steps::claim_next_planned_step(&c, "run", 0, 5)
            .expect("claim")
            .as_deref(),
        Some(consumed.step_id.as_str())
    );
    assert!(crate::role_routing::proposals::consume_approval(
        &c,
        &crate::role_routing::proposals::Approval {
            proposal_id: receipt.id,
            candidate_id: receipt.candidate_id,
        },
        "p",
        0,
        true,
        6,
    )
    .is_err());
}
#[test]
fn rr_05_normal_turn_advances_two_steps_without_persisting_the_draft_body() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');")
        .expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    c.execute_batch("INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1); INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES('run','c','p',0,'responding','reasoning','text','visual',1,''); INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('front','run',0,0,'front-actor','frontend','running','{}','{}',1),('reason','run',0,1,'reason-actor','respond','planned','{}','{}',NULL);")
        .expect("root and plan");

    assert!(advance_provider_step(&c, "run", "private acknowledgement", 2).expect("advance"));
    let statuses = c
        .prepare("SELECT status FROM rr_steps WHERE root_id='run' ORDER BY ordinal")
        .expect("statement")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("statuses");
    assert_eq!(statuses, vec!["succeeded", "running"]);
    let payload: String = c
        .query_row(
            "SELECT payload_json FROM rr_outputs WHERE step_id='front'",
            [],
            |row| row.get(0),
        )
        .expect("digest output");
    assert!(!payload.contains("private acknowledgement"));
    assert!(!advance_provider_step(&c, "run", "final answer", 3).expect("final remains"));
}
#[test]
fn rr_39_host_receipt_p95_is_under_fifty_milliseconds() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE conversations(id TEXT PRIMARY KEY); CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT); CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT); INSERT INTO conversations VALUES('c');")
        .expect("base");
    crate::role_routing::schema::migrate(&c).expect("routing schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
    let mut policy = RoleRoutingSettings {
        enabled: true,
        ..Default::default()
    };
    policy.actors.push(RoutingActor {
        id: "local".into(),
        label: "Local".into(),
        aliases: vec![],
        transport: "provider".into(),
        provider_id: Some("local".into()),
        model: None,
        location: "local".into(),
        resource_group: "gpu".into(),
        max_input_bytes: 4096,
        capabilities: vec!["reason".into()],
    });
    policy.roles.reasoner = Some("local".into());
    policy.recipes.push(RoutingRecipe {
        id: "direct".into(),
        action: crate::role_routing::contracts::RoutingAction::Respond,
        roles: vec!["reasoner".into()],
        enabled: true,
    });
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,?1,'d',1)",
        [serde_json::to_string(&policy).expect("policy")],
    )
    .expect("policy row");
    let mut samples = Vec::new();
    for index in 0..105 {
        let run_id = format!("perf-{index}");
        let message_id = format!("message-{index}");
        c.execute(
            "INSERT INTO conversation_messages VALUES(?1,'c','user','hello','1')",
            [&message_id],
        )
        .expect("message");
        c.execute(
            "INSERT INTO runtime_runs VALUES(?1,'c','conversation.respond',?2)",
            rusqlite::params![run_id, message_id],
        )
        .expect("runtime run");
        let started = std::time::Instant::now();
        assert!(record_provider_turn_start(&c, &run_id, "c", index).expect("receipt"));
        let elapsed = started.elapsed().as_micros();
        if index >= 5 {
            samples.push(elapsed);
        }
        c.execute(
            "UPDATE rr_steps SET status='succeeded',completed_at_ms=?1 WHERE root_id=?2 AND status='running'",
            rusqlite::params![index, run_id],
        )
        .expect("settle step");
        c.execute(
            "UPDATE rr_roots SET phase='completed',active_slot=NULL WHERE root_id=?1",
            [&run_id],
        )
        .expect("settle root");
    }
    samples.sort_unstable();
    let p95 = samples[((samples.len() as f64 * 0.95).ceil() as usize) - 1];
    eprintln!("rr_39_host_receipt_p95_us={p95}");
    assert!(p95 <= 50_000, "routing receipt p95 was {p95}µs");
}
#[test]
fn rr_09_activity_has_no_payload_and_stops_at_terminal_root() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');")
        .expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','p','responding','text','visual',1,'')", [])
        .expect("root");
    assert!(record_actor_activity(&c, "r", "provider_started", 2).expect("activity"));
    let event: (String, String) = c
        .query_row(
            "SELECT kind,data_json FROM rr_events WHERE root_id='r'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("event");
    assert_eq!(event, ("activity".into(), "{}".into()));
    c.execute(
        "UPDATE rr_roots SET phase='completed' WHERE root_id='r'",
        [],
    )
    .expect("complete");
    assert!(!record_actor_activity(&c, "r", "provider_progress", 3).expect("terminal"));
}
#[test]
fn rr_12_finish_requires_a_confirmed_step_transition() {
    let c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');")
        .expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning");
    c.execute(
        "INSERT INTO rr_policy_versions VALUES('p',1,'{}','d',1)",
        [],
    )
    .expect("policy");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('missing-step','c','p','responding','text','visual',1,'')", [])
        .expect("root without step");
    assert!(record_provider_turn_finish(&c, "missing-step", "failed", None, 2).is_err());
    assert_eq!(
        c.query_row(
            "SELECT phase FROM rr_roots WHERE root_id='missing-step'",
            [],
            |row| row.get::<_, String>(0)
        )
        .expect("phase"),
        "responding"
    );

    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES('held','c','p','draining','draining','text','visual',2,'')", [])
        .expect_err("only one active root per conversation");
    c.execute(
        "UPDATE rr_roots SET phase='failed',active_slot=NULL WHERE root_id='missing-step'",
        [],
    )
    .expect("release active slot");
    c.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES('held','c','p','draining','draining','text','visual',2,'')", [])
        .expect("held root");
    c.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json) VALUES('held-step','held',0,0,'actor','respond','succeeded','{}','{}')", [])
        .expect("settled step");
    assert!(record_provider_turn_finish(&c, "held", "failed", None, 3).is_err());
    assert_eq!(
        c.query_row(
            "SELECT phase FROM rr_roots WHERE root_id='held'",
            [],
            |row| { row.get::<_, String>(0) }
        )
        .expect("held phase"),
        "draining"
    );
}
#[test]
fn rr_04_receipt_rows_rollback_with_the_runtime_transaction() {
    let mut c = Connection::open_in_memory().expect("db");
    c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
    crate::role_routing::schema::migrate(&c).expect("schema");
    crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
    crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
    let mut policy = RoleRoutingSettings {
        enabled: true,
        ..Default::default()
    };
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
        record_provider_turn_start_in_transaction(&tx, "run", "c", "text", None, "visual", 1)
            .expect("receipt");
        let receipt: (String, String, Option<i64>, String) = tx
            .query_row(
                "SELECT r.phase,s.status,r.deadline_at_ms,s.config_fingerprint FROM rr_roots r JOIN rr_steps s ON s.root_id=r.root_id WHERE r.root_id='run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("queued receipt");
        assert_eq!(receipt.0, "queued");
        assert_eq!(receipt.1, "planned");
        assert_eq!(receipt.2, None);
        assert_eq!(receipt.3.len(), 64);
        assert!(receipt.3.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // Dropping instead of committing is the failure path that must leave no half root.
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM rr_roots", [], |r| r.get::<_, i64>(0))
            .expect("roots"),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM conversation_messages", [], |r| r
            .get::<_, i64>(0))
            .expect("messages"),
        0
    );
}
