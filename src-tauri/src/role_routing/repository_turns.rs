use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;

struct DispatchSelection {
    candidate: crate::role_routing::selection::Candidate,
    eligible: Vec<crate::role_routing::selection::Candidate>,
    eligible_ids: Vec<String>,
    selection_mode: &'static str,
    policy_revision: i64,
}

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
    let selection = select_dispatch_candidate(connection, &policy, now_ms)?;
    let candidate = &selection.candidate;
    let decision_id = format!("rr-decision-{run_id}");
    let step_id = format!("rr-step-{run_id}-0");
    let candidates = candidate_receipt(&selection.eligible);
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT OR IGNORE INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,deadline_at_ms,scope_digest)
         VALUES(?1,?2,?3,?4,0,'responding','reasoning','text','visual',?5,?6,'')",
        params![run_id, conversation_id, run_id, policy_id, now_ms, now_ms.saturating_add(policy.limits.root_timeout_ms.min(i64::MAX as u64) as i64)],
    ).map_err(|error| error.to_string())?;
    crate::adaptive_improvement::record_decision(
        &transaction,
        &crate::adaptive_improvement::DecisionObservation {
            id: format!("ai-provider-{run_id}"),
            domain: crate::adaptive_improvement::Domain::ProviderRecipe,
            scope_key: "conversation.respond".into(),
            event_seq: 0,
            policy_revision: selection.policy_revision,
            candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(
                &selection.eligible_ids,
            ),
            eligible_candidates: selection.eligible_ids.clone(),
            selected: candidate.recipe_id.clone(),
            selection_mode: selection.selection_mode.into(),
            source_refs_json: json!({"runtimeRunId": run_id, "conversationId": conversation_id})
                .to_string(),
        },
        now_ms,
    )?;
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
    let queued_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM rr_roots WHERE conversation_id=?1 AND phase='queued'",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if queued_count >= i64::from(policy.limits.max_queued_inputs) {
        return Err("Role-routing input queue is full".into());
    }
    let input_message_id: String = transaction.query_row("SELECT input_message_id FROM runtime_runs WHERE id=?1 AND conversation_id=?2 AND route_kind='conversation.respond'",params![run_id,conversation_id],|r|r.get(0)).map_err(|e|e.to_string())?;
    let input_content: String = transaction
        .query_row(
            "SELECT content FROM conversation_messages WHERE id=?1",
            [&input_message_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let selection = select_dispatch_candidate(transaction, &policy, now_ms)?;
    let candidate = &selection.candidate;
    let decision_id = format!("rr-decision-{run_id}");
    let step_id = format!("rr-step-{run_id}-0");
    let candidates = candidate_receipt(&selection.eligible);
    transaction.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,deadline_at_ms,scope_digest) VALUES(?1,?2,?3,?4,0,'queued',NULL,'text','visual',?5,?6,'')",params![run_id,conversation_id,run_id,policy_id,now_ms,now_ms.saturating_add(policy.limits.root_timeout_ms.min(i64::MAX as u64) as i64)]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,disposition,received_at_ms) VALUES(?1,?2,?3,?4,'','text','accepted',?5)",params![format!("rr-input-{run_id}"),run_id,conversation_id,input_message_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_decisions(id,root_id,revision,input_id,features_json,candidates_json,selected_id,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES(?1,?2,0,?3,?4,?5,?6,'respond','[\"rules\"]','rules-v1',?7,?8)",params![decision_id,run_id,format!("rr-input-{run_id}"),feature_snapshot(&input_content,policy.limits.max_reasoning_steps).to_string(),candidates.to_string(),candidate.recipe_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES(?1,?2,?3,0,0,?4,'respond','planned','{}','{}',NULL)",params![step_id,run_id,decision_id,candidate.actor_ids.first().cloned().unwrap_or_default()]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,1,'input_accepted','{}',?2)",params![run_id,now_ms]).map_err(|e|e.to_string())?;
    crate::adaptive_improvement::record_decision(
        transaction,
        &crate::adaptive_improvement::DecisionObservation {
            id: format!("ai-provider-{run_id}"),
            domain: crate::adaptive_improvement::Domain::ProviderRecipe,
            scope_key: "conversation.respond".into(),
            event_seq: 0,
            policy_revision: selection.policy_revision,
            candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(&selection.eligible_ids),
            eligible_candidates: selection.eligible_ids.clone(),
            selected: candidate.recipe_id.clone(),
            selection_mode: selection.selection_mode.into(),
            source_refs_json: json!({"runtimeRunId": run_id, "conversationId": conversation_id, "inputMessageId": input_message_id}).to_string(),
        },
        now_ms,
    )?;
    Ok(true)
}

fn feature_snapshot(input: &str, remaining_steps: u8) -> serde_json::Value {
    let bucket = match input.len() {
        0..=256 => "short",
        257..=4096 => "medium",
        _ => "long",
    };
    json!({"origin":"text","trigger":"initial","priorActorRole":"reasoner","hasActiveWork":false,"inputBytesBucket":bucket,"contextHealth":"green","toolNeed":"unknown","worldState":"unknown","remainingSteps":remaining_steps,"cloudAllowed":true})
}

/// Uses the same finite, single-actor candidate set as conversation dispatch. The receipt cannot
/// claim a learned recipe that the runtime did not actually send to a provider.
fn select_dispatch_candidate(
    connection: &Connection,
    policy: &crate::role_routing::RoleRoutingSettings,
    now_ms: i64,
) -> Result<DispatchSelection, String> {
    let mut eligible = crate::role_routing::selection::candidates_for_action(
        policy,
        crate::role_routing::contracts::RoutingAction::Respond,
    )
    .into_iter()
    .filter(|candidate| candidate.exclusion_reason.is_none() && candidate.actor_ids.len() == 1)
    .collect::<Vec<_>>();
    eligible.sort_by(|left, right| left.recipe_id.cmp(&right.recipe_id));
    let rules = eligible
        .first()
        .cloned()
        .ok_or_else(|| "No single-actor role-routing response recipe is configured".to_string())?;
    if !policy.adaptive_improvement.enabled || !policy.adaptive_improvement.provider_recipe {
        return Ok(DispatchSelection {
            candidate: rules,
            eligible: eligible.clone(),
            eligible_ids: candidate_ids(&eligible),
            selection_mode: "rules",
            policy_revision: 0,
        });
    }
    let eligible_recipe_ids = eligible
        .iter()
        .map(|candidate| candidate.recipe_id.clone())
        .collect::<Vec<_>>();
    let (selected, selection_mode, policy_revision) = crate::adaptive_improvement::choose(
        connection,
        crate::adaptive_improvement::Domain::ProviderRecipe,
        "conversation.respond",
        &eligible_recipe_ids,
        &rules.recipe_id,
        now_ms,
    )?;
    let eligible_ids = candidate_ids(&eligible);
    Ok(DispatchSelection {
        candidate: eligible
            .iter()
            .find(|candidate| candidate.recipe_id == selected)
            .cloned()
            .unwrap_or(rules),
        eligible: eligible.clone(),
        eligible_ids,
        selection_mode,
        policy_revision,
    })
}

fn candidate_ids(candidates: &[crate::role_routing::selection::Candidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.recipe_id.clone())
        .collect()
}

fn candidate_receipt(
    candidates: &[crate::role_routing::selection::Candidate],
) -> serde_json::Value {
    json!(candidates
        .iter()
        .map(|candidate| json!({
            "recipeId": candidate.recipe_id,
            "actorIds": candidate.actor_ids,
            "exclusionReason": candidate.exclusion_reason,
            "reasonCodes": candidate.reason_codes,
        }))
        .collect::<Vec<_>>())
}

pub(crate) fn record_provider_turn_finish(
    connection: &Connection,
    run_id: &str,
    status: &str,
    message_id: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    let phase: Option<String> = connection
        .query_row(
            "SELECT phase FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(phase) = phase else {
        return Ok(());
    };
    // `accept_provider_turn` runs inside the assistant-message transaction first.  The outer
    // runtime finalizer still observes the provider terminal result afterwards, but must not
    // append a second terminal event or overwrite the already adopted root.
    if matches!(phase.as_str(), "completed" | "cancelled" | "failed") {
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
    append_event(
        &transaction,
        run_id,
        if status == "completed" {
            "answer_committed"
        } else {
            "root_finished"
        },
        now_ms,
    )?;
    record_provider_outcome(
        &transaction,
        run_id,
        status == "completed",
        message_id.unwrap_or(run_id),
        0,
        now_ms,
    )?;
    crate::role_routing::learning::repository::mark_root_dirty(&transaction, run_id)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// Atomically adopts a normal provider result when the role-routing root is still current.
/// It is invoked from the assistant-message transaction after the message and scope link exist.
pub(crate) fn accept_provider_turn(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let root: Option<(i64, i64)> = connection
        .query_row(
            "SELECT revision,cancel_requested FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, cancelled)) = root else {
        return Ok(());
    };
    if cancelled != 0 {
        return Err("Role-routing result was cancelled".into());
    }
    let changed = connection
        .execute(
            "UPDATE rr_roots SET phase='completed',result_message_id=?1,active_slot=NULL WHERE root_id=?2 AND phase IN ('responding','draining')",
            params![message_id,run_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing result is stale or already accepted".into());
    }
    connection.execute("UPDATE rr_steps SET status='succeeded',completed_at_ms=?1 WHERE root_id=?2 AND ordinal=0 AND status IN ('planned','running','draining')",params![now_ms,run_id]).map_err(|error|error.to_string())?;
    connection.execute("INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,'answer',?4,1,?5)",params![format!("rr-output-{run_id}"),format!("rr-step-{run_id}-0"),revision,json!({"messageId":message_id}).to_string(),now_ms]).map_err(|error|error.to_string())?;
    append_event(connection, run_id, "answer_committed", now_ms)?;
    record_provider_outcome(connection, run_id, true, message_id, revision, now_ms)?;
    crate::role_routing::learning::repository::mark_root_dirty(connection, run_id)?;
    Ok(())
}

/// The adaptive ledger is optional for legacy databases.  When a receipt did create an adaptive
/// decision, write its terminal technical result in the same database transaction as the routing
/// root so training never sees a completed dispatch without its result.
fn record_provider_outcome(
    connection: &Connection,
    run_id: &str,
    technical_success: bool,
    source_id: &str,
    revision: i64,
    now_ms: i64,
) -> Result<(), String> {
    let decision_id = format!("ai-provider-{run_id}");
    let adaptive_schema_present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_decisions')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !adaptive_schema_present {
        return Ok(());
    }
    let present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_decisions WHERE id=?1)",
            [&decision_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if present {
        crate::adaptive_improvement::record_outcome_in_transaction(
            connection,
            &decision_id,
            Some(technical_success),
            None,
            None,
            None,
            None,
            None,
            None,
            source_id,
            revision,
            now_ms,
        )?;
    }
    Ok(())
}

/// Event sequences are root-local.  The coordinator may append lifecycle events between receipt
/// and terminal adoption, so completion must never assume a fixed sequence number.
fn append_event(
    connection: &Connection,
    root_id: &str,
    kind: &str,
    now_ms: i64,
) -> Result<(), String> {
    let sequence: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM rr_events WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO rr_events(root_id,seq,kind,data_json,created_at_ms) VALUES(?1,?2,?3,'{}',?4)",
            params![root_id, sequence, kind, now_ms],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
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
        crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
        crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
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
            let receipt: (String, String, Option<i64>) = tx
                .query_row(
                    "SELECT r.phase,s.status,r.deadline_at_ms FROM rr_roots r JOIN rr_steps s ON s.root_id=r.root_id WHERE r.root_id='run'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("queued receipt");
            assert_eq!(receipt, ("queued".into(), "planned".into(), Some(180_001)));
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

    #[test]
    fn rr_04_queue_full_leaves_no_input_message() {
        let mut c = Connection::open_in_memory().expect("db");
        c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');").expect("base");
        crate::role_routing::schema::migrate(&c).expect("schema");
        crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
        let mut policy = RoleRoutingSettings::default();
        policy.enabled = true;
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
                record_provider_turn_start_in_transaction(&tx, "run", "c", 2),
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
    fn ai_08_provider_terminal_result_is_recorded_for_the_dispatch_decision() {
        let c = Connection::open_in_memory().expect("db");
        c.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE conversations(id TEXT PRIMARY KEY);CREATE TABLE runtime_runs(id TEXT PRIMARY KEY,conversation_id TEXT,route_kind TEXT,input_message_id TEXT);CREATE TABLE conversation_messages(id TEXT PRIMARY KEY,conversation_id TEXT,role TEXT,content TEXT,created_at TEXT);INSERT INTO conversations VALUES('c');INSERT INTO conversation_messages VALUES('u','c','user','hello','1');INSERT INTO runtime_runs VALUES('run','c','conversation.respond','u');").expect("base");
        crate::role_routing::schema::migrate(&c).expect("schema");
        crate::role_routing::learning::schema::migrate(&c).expect("learning schema");
        crate::adaptive_improvement::migrate(&c).expect("adaptive schema");
        let mut policy = RoleRoutingSettings::default();
        policy.enabled = true;
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
    fn rr_12_acceptance_commits_message_and_root_together() {
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
}
