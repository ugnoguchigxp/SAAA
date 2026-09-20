use crate::persistence::load_role_routing_settings;
use crate::ConversationRouteSettings;
use rusqlite::Connection;

pub(super) fn apply_enabled_role_route(
    connection: &Connection,
    route: &mut ConversationRouteSettings,
) -> Result<(), String> {
    let role_policy = load_role_routing_settings(connection)?;
    if !role_policy.enabled {
        return Ok(());
    }
    let candidate = crate::role_routing::selection::select_rule_candidate(
        &role_policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .ok_or_else(|| "No eligible role-routing response recipe is configured".to_string())?;
    let actor_id = candidate
        .actor_ids
        .first()
        .ok_or_else(|| "Role-routing response recipe has no actor".to_string())?;
    let actor = role_policy
        .actors
        .iter()
        .find(|actor| &actor.id == actor_id)
        .ok_or_else(|| "Role-routing response actor is unavailable".to_string())?;
    if actor.transport != "provider" {
        return Err(
            "The selected role-routing actor is not yet supported for conversation dispatch"
                .to_string(),
        );
    }
    route.source = "provider".to_string();
    route.primary_provider_id = actor.provider_id.clone();
    route.fallback_provider_ids.clear();
    Ok(())
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
        apply_enabled_role_route(&connection, &mut route).expect("role route applies");
        assert_eq!(route.source, "provider");
        assert_eq!(
            route.primary_provider_id.as_deref(),
            Some(DYNAMIC_LAN_PROVIDER_ID)
        );
        assert!(route.fallback_provider_ids.is_empty());
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
