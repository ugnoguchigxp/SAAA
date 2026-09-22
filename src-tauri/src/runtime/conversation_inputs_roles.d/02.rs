#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::settings::save_settings_documents_to_connection;
    use crate::test_support::default_settings_input;
    use crate::{initialize_database, DYNAMIC_LAN_PROVIDER_ID};
    use rusqlite::Connection;
    use serde_json::json;

    fn test_now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as i64
    }

    fn unknown_reachability() -> crate::providers::reachability::ReachabilitySnapshot {
        crate::providers::reachability::ReachabilitySnapshot::default()
    }

    fn accept_receipt(connection: &mut Connection, run_id: &str, now_ms: i64) {
        let transaction = connection.transaction().expect("receipt transaction");
        let policy = crate::persistence::load_role_routing_settings(&transaction).expect("policy");
        let input = crate::role_routing::availability::selection_input_for(
            &policy,
            &unknown_reachability(),
        );
        assert!(
            crate::role_routing::repository::record_provider_turn_start_in_transaction(
                &transaction,
                run_id,
                "conversation_primary",
                "text",
                None,
                "visual",
                now_ms,
                &input,
            )
            .expect("receipt")
        );
        crate::role_routing::coordinator::apply_in_transaction(
            &transaction,
            run_id,
            crate::role_routing::reducer::Event::Start,
            now_ms,
        )
        .expect("claim accepted root");
        transaction.commit().expect("commit receipt");
    }

    #[test]
    fn enabled_direct_recipe_overrides_the_legacy_conversation_route() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let policy = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy");
        policy.value_json = json!({
            "schemaVersion": 1, "enabled": true,
            "actors": [{"id":"qwen","label":"Qwen","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"gpu","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"qwen","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
            "recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"per_request",
            "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("settings save");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing settings")
            .conversation_respond;
        assert_eq!(
            apply_enabled_role_route(&connection, None, &mut route, &unknown_reachability()).expect("role route applies"),
            Some(RoleDispatch::Provider {
                max_input_bytes: 4096
            })
        );
        assert_eq!(route.source, "provider");
        assert_eq!(
            route.primary_provider_id.as_deref(),
            Some(DYNAMIC_LAN_PROVIDER_ID)
        );
        assert!(route.fallback_provider_ids.is_empty());
        assert_eq!(route.timeout_ms, 180_000);
        assert_eq!(route.attempt_timeout_ms, Some(60_000));
    }

    #[test]
    fn codex_actor_selects_the_isolated_dispatch_without_rewriting_provider_settings() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let codex = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.agent" && document.key == "codex-sdk")
            .expect("Codex settings");
        codex.value_json["enabled"] = json!(true);
        codex.value_json["health"] = json!("ready");
        codex.value_json["model"] = json!("gpt-5.6-sol");
        let policy = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy");
        policy.value_json = json!({
            "schemaVersion": 1, "enabled": true,
            "actors": [{"id":"sol","label":"Sol","aliases":[],"transport":"codex_sdk","providerId":null,"model":"gpt-5.6-sol","location":"cloud","resourceGroup":"codex","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"sol","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
            "recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("settings save");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        let prior = route.primary_provider_id.clone();
        assert_eq!(
            apply_enabled_role_route(&connection, None, &mut route, &unknown_reachability()).expect("codex route"),
            Some(RoleDispatch::CodexSdk {
                model: "gpt-5.6-sol".into(),
                max_input_bytes: 4096,
                binding: None,
            })
        );
        assert_eq!(route.primary_provider_id, prior);

        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('codex-input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('codex-run','conversation_primary','conversation.respond','running','codex-input','1')", []).expect("run");
        accept_receipt(&mut connection, "codex-run", test_now_ms());
        let dispatch = apply_enabled_role_route(&connection, Some("codex-run"), &mut route, &unknown_reachability())
            .expect("bound Codex route")
            .expect("dispatch");
        let RoleDispatch::CodexSdk { binding, .. } = dispatch else {
            panic!("expected Codex dispatch");
        };
        let binding = binding.expect("persisted step binding");
        assert_eq!(binding.step_id, "rr-step-codex-run-0");
        assert_eq!(binding.revision, 0);
        assert!(!binding.config_fingerprint.is_empty());
        assert_eq!(binding.purpose, "respond");
    }

    #[test]
    fn rr_26_consumed_premium_step_reaches_the_bound_executor() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let codex = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.agent" && document.key == "codex-sdk")
            .expect("Codex settings");
        codex.value_json["enabled"] = json!(true);
        codex.value_json["health"] = json!("ready");
        codex.value_json["model"] = json!("gpt-6-astra");
        let policy = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy");
        policy.value_json = json!({
            "schemaVersion": 1, "enabled": true,
            "actors": [
                {"id":"qwen","label":"Qwen","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"gpu","maxInputBytes":4096,"capabilities":["reason"]},
                {"id":"astra","label":"Astra","aliases":[],"transport":"codex_sdk","providerId":null,"model":"gpt-6-astra","location":"cloud","resourceGroup":"cloud","maxInputBytes":8192,"capabilities":["reason"]}
            ],
            "roles":{"frontend":null,"reasoner":"qwen","advanced":null,"reviewer":null,"premium":"astra","toolSpecialist":null},
            "recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"per_request",
            "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("settings save");
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('premium-input','conversation_primary','user','check','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('premium-run','conversation_primary','conversation.respond','running','premium-input','1')", []).expect("run");
        let now = test_now_ms();
        accept_receipt(&mut connection, "premium-run", now);
        let policy_id: String = connection
            .query_row(
                "SELECT policy_id FROM rr_roots WHERE root_id='premium-run'",
                [],
                |row| row.get(0),
            )
            .expect("policy id");
        connection.execute("UPDATE rr_steps SET status='succeeded',completed_at_ms=?1 WHERE root_id='premium-run' AND status='running'", [now]).expect("finish base step");
        connection.execute(
            "INSERT INTO rr_premium_proposals(id,root_id,candidate_id,policy_id,revision,expires_at_ms,status,created_at_ms,approved_at_ms,consumed_at_ms) VALUES('premium-proposal','premium-run','astra',?1,0,?2,'approved',?3,?3,?3)",
            rusqlite::params![policy_id, now + 60_000, now],
        ).expect("consumed approval");
        connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('premium-step','premium-run',0,1,'astra','reconsider','running','premium-fp','{}',?1)", [now]).expect("premium step");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        let dispatch = apply_enabled_role_route(&connection, Some("premium-run"), &mut route, &unknown_reachability())
            .expect("premium route")
            .expect("dispatch");
        let RoleDispatch::CodexSdk { model, binding, .. } = dispatch else {
            panic!("expected premium Codex dispatch");
        };
        assert_eq!(model, "gpt-6-astra");
        let binding = binding.expect("binding");
        assert_eq!(binding.step_id, "premium-step");
        assert_eq!(binding.purpose, "reconsider");
    }

    #[test]
    fn rr_03_queued_root_uses_its_immutable_policy_receipt() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let policy = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy");
        policy.value_json = json!({
            "schemaVersion": 1, "enabled": true,
            "actors": [{"id":"qwen","label":"Qwen","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"gpu","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"qwen","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
            "recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("settings save");
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('immutable-input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('immutable-run','conversation_primary','conversation.respond','running','immutable-input','1')", []).expect("run");
        let started_at = test_now_ms();
        accept_receipt(&mut connection, "immutable-run", started_at);

        documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy")
            .value_json["limits"]["maxReasoningSteps"] = json!(3);
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("save a newer enabled policy");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        assert_eq!(
            apply_enabled_role_route(&connection, Some("immutable-run"), &mut route, &unknown_reachability())
                .expect("receipt route"),
            Some(RoleDispatch::Provider {
                max_input_bytes: 4096
            })
        );
        assert_eq!(
            route.primary_provider_id.as_deref(),
            Some(DYNAMIC_LAN_PROVIDER_ID)
        );
    }

    #[test]
    fn rr_04_receipt_and_rr_12_completion_are_persisted_once() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let policy = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("policy");
        policy.value_json = json!({
            "schemaVersion":1,"enabled":true,
            "actors":[{"id":"qwen","label":"Qwen","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"gpu","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"qwen","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},"recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},"speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},"selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},"premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("save policy");
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('run','conversation_primary','conversation.respond','running','input','1')", []).expect("run");
        let started_at = test_now_ms();
        accept_receipt(&mut connection, "run", started_at);
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('answer','conversation_primary','assistant','answer','2')", []).expect("answer");
        crate::role_routing::repository::record_provider_turn_finish(
            &connection,
            "run",
            "completed",
            Some("answer"),
            started_at + 1,
        )
        .expect("finish");
        crate::role_routing::repository::record_provider_turn_finish(
            &connection,
            "run",
            "completed",
            Some("answer"),
            started_at + 2,
        )
        .expect("duplicate finish");
        let outputs: i64 = connection
            .query_row(
                "SELECT count(*) FROM rr_outputs WHERE step_id='rr-step-run-0'",
                [],
                |row| row.get(0),
            )
            .expect("outputs");
        assert_eq!(outputs, 1);
    }

    #[test]
    fn rr_22_role_route_enforces_the_step_budget() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("policy")
            .value_json = json!({
            "schemaVersion":1,"enabled":true,
            "actors":[{"id":"qwen","label":"Qwen","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"gpu","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"qwen","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},"recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":1,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},"speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},"selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},"premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("save policy");
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('budget-input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('budget-run','conversation_primary','conversation.respond','running','budget-input','1')", []).expect("run");
        let started_at = test_now_ms();
        accept_receipt(&mut connection, "budget-run", started_at);
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        // The single running step is the already-reserved first dispatch and remains valid.
        assert_eq!(
            apply_enabled_role_route(&connection, Some("budget-run"), &mut route, &unknown_reachability())
                .expect("first dispatch fits the budget"),
            Some(RoleDispatch::Provider {
                max_input_bytes: 4096
            })
        );
        connection
            .execute(
                "INSERT INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms,completed_at_ms) SELECT 'budget-spent',root_id,decision_id,revision,1,actor_id,purpose,'succeeded',config_fingerprint,'{}',?1,?1 FROM rr_steps WHERE id='rr-step-budget-run-0'",
                [started_at],
            )
            .expect("spent step");
        assert!(apply_enabled_role_route(&connection, Some("budget-run"), &mut route, &unknown_reachability()).is_err());
    }

    #[test]
    fn rr_22_cloud_revoked_before_dispatch() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let codex = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.agent" && document.key == "codex-sdk")
            .expect("Codex settings");
        codex.value_json["enabled"] = json!(true);
        codex.value_json["health"] = json!("ready");
        codex.value_json["model"] = json!("gpt-5.6-sol");
        documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("policy")
            .value_json = json!({
            "schemaVersion":1,"enabled":true,
            "actors":[{"id":"sol","label":"Sol","aliases":[],"transport":"codex_sdk","providerId":null,"model":"gpt-5.6-sol","location":"cloud","resourceGroup":"codex","maxInputBytes":4096,"capabilities":["reason"]}],
            "roles":{"frontend":null,"reasoner":"sol","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
            "recipes":[{"id":"direct","action":"respond","roles":["reasoner"],"enabled":true}],
            "limits":{"maxReasoningSteps":1,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"never","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(&mut connection, &documents).expect("settings save");
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('cloud-input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('cloud-run','conversation_primary','conversation.respond','running','cloud-input','1')", []).expect("run");
        accept_receipt(&mut connection, "cloud-run", test_now_ms());

        let mut codex_document = crate::persistence::settings::read_settings_document(
            &connection,
            "providers.agent",
            "codex-sdk",
        )
        .expect("Codex document");
        codex_document.value_json["enabled"] = json!(false);
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.agent' AND key='codex-sdk'",
                [codex_document.value_json.to_string()],
            )
            .expect("revoke Codex");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        let error = apply_enabled_role_route(&connection, Some("cloud-run"), &mut route, &unknown_reachability())
            .expect_err("revoked cloud actor must not dispatch");
        assert!(error.contains("revoked before dispatch"));
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM provider_sessions", [], |row| row
                    .get::<_, i64>(0))
                .expect("provider sessions"),
            0
        );
    }

    fn install_location_policy(connection: &mut Connection) {
        let mut documents = default_settings_input();
        let providers = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("providers");
        providers.value_json["providers"]
            .as_array_mut()
            .expect("provider list")
            .push(json!({
                "kind": "openai-compatible",
                "id": "cloud-api",
                "enabled": true,
                "label": "Cloud",
                "location": "cloud",
                "endpoint": "https://example.invalid/v1",
                "model": "cloud-model",
                "authentication": "none"
            }));
        documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("policy")
            .value_json = json!({
            "schemaVersion": 1, "enabled": true,
            "actors": [
                {"id":"larm","label":"LARM","aliases":[],"transport":"provider","providerId":DYNAMIC_LAN_PROVIDER_ID,"model":null,"location":"local","resourceGroup":"lan","maxInputBytes":32768,"capabilities":["reason"]},
                {"id":"cloud","label":"Cloud","aliases":[],"transport":"provider","providerId":"cloud-api","model":null,"location":"cloud","resourceGroup":"cloud","maxInputBytes":32768,"capabilities":["reason"]}
            ],
            "roles":{"frontend":null,"reasoner":"larm","advanced":"cloud","reviewer":null,"premium":null,"toolSpecialist":null},
            "recipes":[
                {"id":"10-respond-home","action":"respond","roles":["reasoner"],"enabled":true},
                {"id":"20-respond-away","action":"respond","roles":["advanced"],"enabled":true}
            ],
            "limits":{"maxReasoningSteps":4,"maxToolCalls":32,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":2,"maxEstimatedCostMicros":null},
            "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
            "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
            "premiumApproval":"per_request","learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false}
        });
        save_settings_documents_to_connection(connection, &documents).expect("settings save");
    }

    fn insert_turn(connection: &Connection, run_id: &str) {
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES(?1,'conversation_primary','user','hello','1')", [format!("{run_id}-input")]).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES(?1,'conversation_primary','conversation.respond','running',?2,'1')", rusqlite::params![run_id, format!("{run_id}-input")]).expect("run");
    }

    fn accept_with_snapshot(
        connection: &mut Connection,
        run_id: &str,
        snapshot: &crate::providers::reachability::ReachabilitySnapshot,
    ) {
        let now_ms = test_now_ms();
        let transaction = connection.transaction().expect("tx");
        let policy = crate::persistence::load_role_routing_settings(&transaction).expect("policy");
        let input = crate::role_routing::availability::selection_input_for(&policy, snapshot);
        assert!(crate::role_routing::repository::record_provider_turn_start_in_transaction(
            &transaction,
            run_id,
            "conversation_primary",
            "text",
            None,
            "visual",
            now_ms,
            &input,
        ).expect("receipt"));
        crate::role_routing::coordinator::apply_in_transaction(
            &transaction,
            run_id,
            crate::role_routing::reducer::Event::Start,
            now_ms,
        ).expect("start");
        transaction.commit().expect("commit");
    }

    #[test]
    fn rr_ls_20_unreachable_selects_the_away_recipe() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        install_location_policy(&mut connection);
        insert_turn(&connection, "away-run");
        let observed = crate::providers::reachability::ReachabilityState::default();
        observed.record(false, std::time::Instant::now());
        observed.record(false, std::time::Instant::now());
        accept_with_snapshot(&mut connection, "away-run", &observed.snapshot());
        let selected: String = connection.query_row("SELECT selected_id FROM rr_decisions WHERE root_id='away-run'", [], |row| row.get(0)).expect("selected");
        let reasons: String = connection.query_row("SELECT reason_codes_json FROM rr_decisions WHERE root_id='away-run'", [], |row| row.get(0)).expect("reasons");
        let actor: String = connection.query_row("SELECT actor_id FROM rr_steps WHERE root_id='away-run'", [], |row| row.get(0)).expect("actor");
        assert_eq!(selected, "20-respond-away");
        assert!(reasons.contains("location_fallback"));
        assert_eq!(actor, "cloud");
    }

    #[test]
    fn rr_ls_21_reachable_selects_the_home_recipe() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        install_location_policy(&mut connection);
        insert_turn(&connection, "home-run");
        let observed = crate::providers::reachability::ReachabilityState::default();
        observed.record(true, std::time::Instant::now());
        accept_with_snapshot(&mut connection, "home-run", &observed.snapshot());
        let selected: String = connection.query_row("SELECT selected_id FROM rr_decisions WHERE root_id='home-run'", [], |row| row.get(0)).expect("selected");
        let actor: String = connection.query_row("SELECT actor_id FROM rr_steps WHERE root_id='home-run'", [], |row| row.get(0)).expect("actor");
        assert_eq!(selected, "10-respond-home");
        assert_eq!(actor, "larm");
    }

    #[test]
    fn rr_ls_22_dispatch_refuses_a_lan_actor_that_became_unreachable() {
        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        install_location_policy(&mut connection);
        insert_turn(&connection, "late-run");
        let observed = crate::providers::reachability::ReachabilityState::default();
        observed.record(true, std::time::Instant::now());
        accept_with_snapshot(&mut connection, "late-run", &observed.snapshot());
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        let error = apply_enabled_role_route(
            &connection,
            Some("late-run"),
            &mut route,
            &crate::providers::reachability::ReachabilitySnapshot {
                harness: crate::providers::reachability::Reachability::Unreachable,
                ..crate::providers::reachability::ReachabilitySnapshot::default()
            },
        )
        .expect_err("unreachable harness");
        assert_eq!(error, "Role-routing LAN provider is unreachable before dispatch");
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM provider_sessions", [], |row| row.get::<_, i64>(0))
                .expect("sessions"),
            0
        );
    }
}
