use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;

/// Creates the durable R1 ledger entries only after the legacy runtime run and its input message
/// have committed. Disabled policies leave no role-routing rows, preserving the old path exactly.
pub(crate) fn record_provider_turn_start(
    connection: &Connection,
    run_id: &str,
    conversation_id: &str,
    now_ms: i64,
) -> Result<bool, String> {
    let policy: Option<(String, String)> = connection
        .query_row(
            "SELECT p.id, p.config_json FROM rr_policy_versions p ORDER BY p.version DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((policy_id, policy_json)) = policy else {
        return Ok(false);
    };
    let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
        .map_err(|error| format!("Stored role-routing policy is invalid: {error}"))?;
    if !policy.enabled {
        return Ok(false);
    }
    let input_message_id: String = connection.query_row(
        "SELECT input_message_id FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND route_kind='conversation.respond'",
        params![run_id, conversation_id], |row| row.get(0),
    ).map_err(|error| error.to_string())?;
    let candidate = crate::role_routing::selection::select_rule_candidate(
        &policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .ok_or_else(|| "No eligible role-routing response recipe is configured".to_string())?;
    let decision_id = format!("rr-decision-{run_id}");
    let step_id = format!("rr-step-{run_id}-0");
    let candidates = json!([{
        "recipeId": candidate.recipe_id,
        "actorIds": candidate.actor_ids,
        "exclusionReason": candidate.exclusion_reason,
    }]);
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest)
         VALUES(?1,?2,?3,?4,0,'responding','reasoning','text','visual',?5,'')",
        params![run_id, conversation_id, run_id, policy_id, now_ms],
    ).map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,disposition,received_at_ms)
         VALUES(?1,?2,?3,?4,'','text','accepted',?5)",
        params![format!("rr-input-{run_id}"), run_id, conversation_id, input_message_id, now_ms],
    ).map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_decisions(id,root_id,revision,input_id,features_json,candidates_json,selected_id,action,reason_codes_json,ranker_version,policy_id,created_at_ms)
         VALUES(?1,?2,0,?3,'{}',?4,?5,'respond','[\"rules\"]','rules-v1',?6,?7)",
        params![decision_id, run_id, format!("rr-input-{run_id}"), candidates.to_string(), candidate.recipe_id, policy_id, now_ms],
    ).map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms)
         VALUES(?1,?2,?3,0,0,?4,'respond','running','{}','{}',?5)",
        params![step_id, run_id, decision_id, candidate.actor_ids.first().cloned().unwrap_or_default(), now_ms],
    ).map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,1,'input_accepted','{}',?2)",
        params![run_id, now_ms],
    ).map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(true)
}

/// The receipt path calls this before committing the user message and runtime run. Keeping these
/// inserts in the caller's transaction means a visible user input can never exist without the
/// corresponding routing root when role routing is enabled.
pub(crate) fn record_provider_turn_start_in_transaction(
    transaction: &Transaction<'_>,
    run_id: &str,
    conversation_id: &str,
    now_ms: i64,
) -> Result<bool, String> {
    let policy: Option<(String, String)> = transaction
        .query_row(
            "SELECT id,config_json FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((policy_id, policy_json)) = policy else {
        return Ok(false);
    };
    let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
        .map_err(|e| format!("Stored role-routing policy is invalid: {e}"))?;
    if !policy.enabled {
        return Ok(false);
    }
    let input_message_id: String = transaction.query_row("SELECT input_message_id FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND route_kind='conversation.respond'",params![run_id,conversation_id],|r|r.get(0)).map_err(|e|e.to_string())?;
    let candidate = crate::role_routing::selection::select_rule_candidate(
        &policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .ok_or_else(|| "No eligible role-routing response recipe is configured".to_string())?;
    let decision_id = format!("rr-decision-{run_id}");
    let step_id = format!("rr-step-{run_id}-0");
    let candidates = json!([{ "recipeId":candidate.recipe_id,"actorIds":candidate.actor_ids,"exclusionReason":candidate.exclusion_reason }]);
    transaction.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,scope_digest) VALUES(?1,?2,?3,?4,0,'responding','reasoning','text','visual',?5,'')",params![run_id,conversation_id,run_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,disposition,received_at_ms) VALUES(?1,?2,?3,?4,'','text','accepted',?5)",params![format!("rr-input-{run_id}"),run_id,conversation_id,input_message_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_decisions(id,root_id,revision,input_id,features_json,candidates_json,selected_id,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES(?1,?2,0,?3,'{}',?4,?5,'respond','[\"rules\"]','rules-v1',?6,?7)",params![decision_id,run_id,format!("rr-input-{run_id}"),candidates.to_string(),candidate.recipe_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES(?1,?2,?3,0,0,?4,'respond','running','{}','{}',?5)",params![step_id,run_id,decision_id,candidate.actor_ids.first().cloned().unwrap_or_default(),now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,1,'input_accepted','{}',?2)",params![run_id,now_ms]).map_err(|e|e.to_string())?;
    Ok(true)
}

pub(crate) fn record_provider_turn_finish(
    connection: &Connection,
    run_id: &str,
    status: &str,
    message_id: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_roots WHERE root_id=?1)",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    let step_status = match status {
        "completed" => "succeeded",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let phase = match status {
        "completed" => "completed",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute(
        "UPDATE rr_steps SET status=?1, completed_at_ms=?2 WHERE root_id=?3 AND ordinal=0 AND status IN ('planned','running','draining')",
        params![step_status, now_ms, run_id],
    ).map_err(|error| error.to_string())?;
    if let Some(message_id) = message_id {
        transaction.execute(
            "INSERT OR IGNORE INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,0,'answer',?3,1,?4)",
            params![format!("rr-output-{run_id}"), format!("rr-step-{run_id}-0"), json!({"messageId":message_id}).to_string(), now_ms],
        ).map_err(|error| error.to_string())?;
    }
    transaction
        .execute(
            "UPDATE rr_roots SET phase=?1, result_message_id=?2, active_slot=NULL WHERE root_id=?3",
            params![phase, message_id, run_id],
        )
        .map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,2,?2,'{}',?3)",
        params![run_id, if status == "completed" { "answer_committed" } else { "root_finished" }, now_ms],
    ).map_err(|error| error.to_string())?;
    crate::role_routing::learning::repository::mark_root_dirty(&transaction, run_id)?;
    transaction.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_routing::contracts::{RoleRoutingSettings, RoutingActor, RoutingRecipe};

    #[test]
    fn rr_04_receipt_rows_rollback_with_the_runtime_transaction() {
        let mut c = Connection::open_in_memory().expect("db");
        c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&c).expect("schema");
        let mut policy = RoleRoutingSettings::default();
        policy.enabled = true;
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
            record_provider_turn_start_in_transaction(&tx, "run", "c", 1).expect("receipt");
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
}
