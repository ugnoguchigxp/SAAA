use crate::persistence::load_role_routing_settings;
use crate::ConversationRouteSettings;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoleDispatch {
    Provider {
        max_input_bytes: u32,
    },
    CodexSdk {
        model: String,
        max_input_bytes: u32,
        binding: Option<CodexStepBinding>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexStepBinding {
    pub(crate) step_id: String,
    pub(crate) revision: u32,
    pub(crate) config_fingerprint: String,
    pub(crate) purpose: String,
}

pub(super) fn apply_enabled_role_route(
    connection: &Connection,
    root_id: Option<&str>,
    route: &mut ConversationRouteSettings,
) -> Result<Option<RoleDispatch>, String> {
    let role_policy = load_role_policy_for_root(connection, root_id)?;
    if !role_policy.enabled {
        return Ok(None);
    }
    // Candidate construction is the hard filter. Adaptive improvement may only reorder this
    // exact set, so it cannot enable an unavailable actor.
    let mut eligible = crate::role_routing::selection::candidates_for_action(
        &role_policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .into_iter()
    .filter(|candidate| candidate.exclusion_reason.is_none())
    .collect::<Vec<_>>();
    eligible.sort_by(|left, right| left.recipe_id.cmp(&right.recipe_id));
    let rules_candidate = eligible
        .first()
        .cloned()
        .ok_or_else(|| "No eligible role-routing response recipe is configured".to_string())?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let selected_id = match root_id {
        // The receipt's decision is immutable. Re-running adaptive selection here could dispatch
        // a different actor than the one recorded before the input transaction committed.
        Some(root_id) => connection
            .query_row(
                "SELECT d.selected_id FROM rr_decisions d JOIN rr_roots r ON r.root_id=d.root_id AND r.revision=d.revision WHERE d.root_id=?1 ORDER BY d.created_at_ms DESC LIMIT 1",
                [root_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .flatten()
            .ok_or_else(|| "Role-routing receipt has no selected recipe".to_string())?,
        None if role_policy.adaptive_improvement.enabled
            && role_policy.adaptive_improvement.provider_recipe =>
        {
            let candidate_ids = eligible
                .iter()
                .map(|candidate| candidate.recipe_id.clone())
                .collect::<Vec<_>>();
            crate::adaptive_improvement::choose(
                connection,
                crate::adaptive_improvement::Domain::ProviderRecipe,
                "conversation.respond",
                &candidate_ids,
                &rules_candidate.recipe_id,
                now_ms,
            )?
            .0
        }
        None => rules_candidate.recipe_id.clone(),
    };
    let candidate = eligible
        .into_iter()
        .find(|candidate| candidate.recipe_id == selected_id)
        .ok_or_else(|| "Role-routing receipt selected an ineligible recipe".to_string())?;
    // Compile the selected recipe before dispatch. An unbounded or invalid plan returns an error
    // here, so no provider is started for a recipe the executor cannot run.
    let compiled =
        crate::role_routing::recipe::compile_recipe_by_id(&role_policy, &candidate.recipe_id)?;
    let first_actor_id = candidate
        .actor_ids
        .first()
        .ok_or_else(|| "Role-routing response recipe has no actor".to_string())?;
    // Validate the exact claimed step at the real dispatch boundary. The permit checks revision,
    // deadline and cumulative budget, and prevents a receipt/actor mismatch from reaching I/O.
    let (actor_id, step_binding) = if let Some(root_id) = root_id {
        let permit = crate::role_routing::executor::permit_next_step(
            connection,
            &role_policy,
            root_id,
            now_ms,
        )?
        .ok_or_else(|| "Role-routing root is still queued".to_string())?;
        let planned = compiled
            .steps
            .iter()
            .find(|step| step.ordinal == permit.ordinal);
        let approved_premium = if planned.is_none() && permit.purpose == "reconsider" {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM rr_premium_proposals WHERE root_id=?1 AND candidate_id=?2 AND revision=?3 AND status='approved' AND consumed_at_ms IS NOT NULL)",
                    rusqlite::params![root_id, permit.actor_id, permit.revision],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| error.to_string())?
        } else {
            false
        };
        if !approved_premium && planned.is_none() {
            return Err("Role-routing permit references an unknown plan step".into());
        }
        if planned.is_some_and(|planned| planned.actor_id != permit.actor_id) {
            return Err("Role-routing receipt actor does not match the claimed step".into());
        }
        (
            permit.actor_id,
            Some(CodexStepBinding {
                step_id: permit.step_id,
                revision: permit.revision,
                config_fingerprint: permit.config_fingerprint,
                purpose: permit.purpose,
            }),
        )
    } else {
        (first_actor_id.clone(), None)
    };
    let actor = role_policy
        .actors
        .iter()
        .find(|actor| actor.id == actor_id)
        .ok_or_else(|| "Role-routing response actor is unavailable".to_string())?;
    validate_actor_host(connection, actor)?;
    if actor.transport == "codex_sdk" {
        return Ok(Some(RoleDispatch::CodexSdk {
            model: actor
                .model
                .clone()
                .ok_or_else(|| "Role-routing Codex actor has no model".to_string())?,
            max_input_bytes: actor.max_input_bytes,
            binding: step_binding,
        }));
    }
    if actor.transport != "provider" {
        return Err("Role-routing actor has an unsupported transport".to_string());
    }
    route.source = "provider".to_string();
    route.primary_provider_id = actor.provider_id.clone();
    route.fallback_provider_ids.clear();
    // A role-routing root owns the total budget. Preserve the existing provider implementation,
    // but do not let its legacy route timeout outlive the immutable root receipt.
    route.timeout_ms = role_policy.limits.root_timeout_ms;
    route.attempt_timeout_ms = Some(
        role_policy
            .limits
            .step_timeout_ms
            .min(role_policy.limits.root_timeout_ms),
    );
    Ok(Some(RoleDispatch::Provider {
        max_input_bytes: actor.max_input_bytes,
    }))
}

/// Re-checks mutable host facts at the last synchronous boundary before any provider/SDK I/O.
/// A policy snapshot authorizes an actor identity, but does not freeze provider availability.
fn validate_actor_host(
    connection: &Connection,
    actor: &crate::role_routing::contracts::RoutingActor,
) -> Result<(), String> {
    match actor.transport.as_str() {
        "provider" => {
            let provider_id = actor
                .provider_id
                .as_deref()
                .ok_or_else(|| "Role-routing provider actor has no provider id".to_string())?;
            let providers = crate::persistence::load_model_providers(connection)?;
            let provider = providers
                .providers
                .iter()
                .find(|provider| provider.id() == provider_id && provider.enabled())
                .ok_or_else(|| "Role-routing provider was revoked before dispatch".to_string())?;
            if provider.location() != actor.location {
                return Err("Role-routing provider location changed before dispatch".into());
            }
            if !matches!(
                provider,
                crate::ModelProviderSettings::OpenAiCompatible(_)
                    | crate::ModelProviderSettings::AgentSession(_)
                    | crate::ModelProviderSettings::DynamicLan(_)
            ) {
                return Err(
                    "Role-routing provider no longer supports conversation inference".into(),
                );
            }
            Ok(())
        }
        "codex_sdk" => {
            let codex = crate::persistence::load_codex_settings(connection)?;
            if !codex.enabled || codex.health != "ready" {
                return Err("Role-routing Codex actor was revoked before dispatch".into());
            }
            if actor.model.as_deref() != Some(codex.model.as_str()) {
                return Err("Role-routing Codex model changed before dispatch".into());
            }
            Ok(())
        }
        _ => Err("Role-routing actor has an unsupported transport".into()),
    }
}

/// A receipt owns its policy for its full lifetime. In particular, a queued root must not pick up
/// a settings edit made after it was accepted but before it is claimed for dispatch.
fn load_role_policy_for_root(
    connection: &Connection,
    root_id: Option<&str>,
) -> Result<crate::role_routing::RoleRoutingSettings, String> {
    let receipt_policy: Option<String> = match root_id {
        Some(root_id) => connection
            .query_row(
                "SELECT p.config_json
                 FROM rr_roots r
                 JOIN rr_policy_versions p ON p.id = r.policy_id
                 WHERE r.root_id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        None => None,
    };
    match receipt_policy {
        Some(policy_json) => serde_json::from_str(&policy_json)
            .map_err(|error| format!("Role-routing receipt policy is invalid: {error}")),
        None => load_role_routing_settings(connection),
    }
}

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
            apply_enabled_role_route(&connection, None, &mut route).expect("role route applies"),
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
            apply_enabled_role_route(&connection, None, &mut route).expect("codex route"),
            Some(RoleDispatch::CodexSdk {
                model: "gpt-5.6-sol".into(),
                max_input_bytes: 4096,
                binding: None,
            })
        );
        assert_eq!(route.primary_provider_id, prior);

        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('codex-input','conversation_primary','user','hello','1')", []).expect("input");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('codex-run','conversation_primary','conversation.respond','running','codex-input','1')", []).expect("run");
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "codex-run",
            "conversation_primary",
            test_now_ms(),
        )
        .expect("receipt"));
        let dispatch = apply_enabled_role_route(&connection, Some("codex-run"), &mut route)
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "premium-run",
            "conversation_primary",
            now,
        )
        .expect("receipt"));
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
        let dispatch = apply_enabled_role_route(&connection, Some("premium-run"), &mut route)
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "immutable-run",
            "conversation_primary",
            started_at,
        )
        .expect("receipt"));

        documents
            .iter_mut()
            .find(|document| document.namespace == "routing.roles")
            .expect("role policy")
            .value_json = json!({"schemaVersion": 1, "enabled": false});
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("disable new roots");
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        assert_eq!(
            apply_enabled_role_route(&connection, Some("immutable-run"), &mut route)
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "run",
            "conversation_primary",
            started_at
        )
        .expect("start"));
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "budget-run",
            "conversation_primary",
            started_at
        )
        .expect("receipt"));
        let mut route = crate::persistence::load_routing_settings(&connection)
            .expect("routing")
            .conversation_respond;
        // The single running step is the already-reserved first dispatch and remains valid.
        assert_eq!(
            apply_enabled_role_route(&connection, Some("budget-run"), &mut route)
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
        assert!(apply_enabled_role_route(&connection, Some("budget-run"), &mut route).is_err());
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "cloud-run",
            "conversation_primary",
            test_now_ms(),
        )
        .expect("receipt"));

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
        let error = apply_enabled_role_route(&connection, Some("cloud-run"), &mut route)
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
}
