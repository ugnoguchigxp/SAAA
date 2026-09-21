use crate::persistence::load_role_routing_settings;
use crate::ConversationRouteSettings;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoleDispatch {
    Provider,
    CodexSdk { model: String, max_input_bytes: u32 },
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
    // exact set, so it cannot enable an unavailable actor or a multi-step recipe this runtime
    // does not execute.
    let mut eligible = crate::role_routing::selection::candidates_for_action(
        &role_policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .into_iter()
    .filter(|candidate| candidate.exclusion_reason.is_none() && candidate.actor_ids.len() == 1)
    .collect::<Vec<_>>();
    eligible.sort_by(|left, right| left.recipe_id.cmp(&right.recipe_id));
    let rules_candidate = eligible
        .first()
        .cloned()
        .ok_or_else(|| "No single-actor role-routing response recipe is configured".to_string())?;
    let candidate_ids = eligible
        .iter()
        .map(|candidate| candidate.recipe_id.clone())
        .collect::<Vec<_>>();
    let selected_id = if role_policy.adaptive_improvement.enabled
        && role_policy.adaptive_improvement.provider_recipe
    {
        crate::adaptive_improvement::choose(
            connection,
            crate::adaptive_improvement::Domain::ProviderRecipe,
            "conversation.respond",
            &candidate_ids,
            &rules_candidate.recipe_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0),
        )?
        .0
    } else {
        rules_candidate.recipe_id.clone()
    };
    let candidate = eligible
        .into_iter()
        .find(|candidate| candidate.recipe_id == selected_id)
        .unwrap_or(rules_candidate);
    let actor_id = candidate
        .actor_ids
        .first()
        .ok_or_else(|| "Role-routing response recipe has no actor".to_string())?;
    let actor = role_policy
        .actors
        .iter()
        .find(|actor| &actor.id == actor_id)
        .ok_or_else(|| "Role-routing response actor is unavailable".to_string())?;
    if actor.transport == "codex_sdk" {
        return Ok(Some(RoleDispatch::CodexSdk {
            model: actor
                .model
                .clone()
                .ok_or_else(|| "Role-routing Codex actor has no model".to_string())?,
            max_input_bytes: actor.max_input_bytes,
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
    Ok(Some(RoleDispatch::Provider))
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
            Some(RoleDispatch::Provider)
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
            })
        );
        assert_eq!(route.primary_provider_id, prior);
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "immutable-run",
            "conversation_primary",
            1,
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
            Some(RoleDispatch::Provider)
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
        assert!(crate::role_routing::repository::record_provider_turn_start(
            &connection,
            "run",
            "conversation_primary",
            1
        )
        .expect("start"));
        connection.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('answer','conversation_primary','assistant','answer','2')", []).expect("answer");
        crate::role_routing::repository::record_provider_turn_finish(
            &connection,
            "run",
            "completed",
            Some("answer"),
            2,
        )
        .expect("finish");
        crate::role_routing::repository::record_provider_turn_finish(
            &connection,
            "run",
            "completed",
            Some("answer"),
            3,
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
}
