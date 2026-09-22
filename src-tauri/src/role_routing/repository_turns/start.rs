use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use sha2::{Digest, Sha256};

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
    let planned_steps = compile_selected_plan(&policy, candidate)?;
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
    if let Some(rules) = selection.eligible.first() {
        crate::role_routing::ranker::record_shadow_observation(
            &transaction,
            &decision_id,
            &policy,
            &selection.eligible,
            rules,
            now_ms,
        )?;
    }
    for (index, step) in planned_steps.iter().enumerate() {
        let step_id = format!("rr-step-{run_id}-{}", step.ordinal);
        let status = if index == 0 { "running" } else { "planned" };
        let started_at = if index == 0 { Some(now_ms) } else { None };
        let config_fingerprint = step_config_fingerprint(&policy_json, candidate, step)?;
        transaction.execute(
            "INSERT OR IGNORE INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms)
             VALUES(?1,?2,?3,0,?4,?5,?6,?7,?8,'{}',?9)",
            params![step_id, run_id, decision_id, step.ordinal, step.actor_id, step.purpose, status, config_fingerprint, started_at],
        ).map_err(|error| error.to_string())?;
    }
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
    origin: &str,
    source_id: Option<&str>,
    presentation_mode: &str,
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
    let planned_steps = compile_selected_plan(&policy, candidate)?;
    let candidates = candidate_receipt(&selection.eligible);
    transaction.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,deadline_at_ms,scope_digest) VALUES(?1,?2,?3,?4,0,'queued',NULL,?5,?6,?7,NULL,'')",params![run_id,conversation_id,run_id,policy_id,origin,presentation_mode,now_ms]).map_err(|e|e.to_string())?;
    if let Some(reasoning_request_id) =
        source_id.filter(|id| crate::larm_voice::frontdesk_repository::is_reasoning_request_id(id))
    {
        // Adopt every ASR segment that formed this verbatim reasoning request. The LFM receipt is
        // written before inference; attaching it here makes the same Role Routing root own the
        // complete voice history without asking LFM to rewrite user authority.
        transaction.execute(
            "UPDATE rr_inputs SET root_id=?1,disposition='adopted'
             WHERE root_id IS NULL AND conversation_id=?2 AND message_id IN (
               SELECT u.user_message_id FROM lfm_voice_utterances u
               WHERE u.conversation_id=?2
                 AND u.rowid <= (SELECT rowid FROM lfm_voice_utterances WHERE reasoning_request_id=?3)
                 AND u.rowid > COALESCE((
                   SELECT MAX(previous.rowid) FROM lfm_voice_utterances previous
                   WHERE previous.conversation_id=?2 AND previous.status='delegate'
                     AND previous.rowid < (SELECT rowid FROM lfm_voice_utterances WHERE reasoning_request_id=?3)
                 ),0)
             )",
            params![run_id, conversation_id, reasoning_request_id],
        ).map_err(|e|e.to_string())?;
    }
    let payload_digest = format!("{:x}", Sha256::digest(input_content.as_bytes()));
    transaction.execute("INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,source_id,disposition,received_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,'accepted',?8)",params![format!("rr-input-{run_id}"),run_id,conversation_id,input_message_id,payload_digest,origin,source_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_decisions(id,root_id,revision,input_id,features_json,candidates_json,selected_id,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES(?1,?2,0,?3,?4,?5,?6,'respond','[\"rules\"]','rules-v1',?7,?8)",params![decision_id,run_id,format!("rr-input-{run_id}"),feature_snapshot(&input_content,policy.limits.max_reasoning_steps).to_string(),candidates.to_string(),candidate.recipe_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
    if let Some(rules) = selection.eligible.first() {
        crate::role_routing::ranker::record_shadow_observation(
            transaction,
            &decision_id,
            &policy,
            &selection.eligible,
            rules,
            now_ms,
        )?;
    }
    for step in planned_steps.iter() {
        let step_id = format!("rr-step-{run_id}-{}", step.ordinal);
        let config_fingerprint = step_config_fingerprint(&policy_json, candidate, step)?;
        transaction.execute("INSERT INTO rr_steps(id,root_id,decision_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES(?1,?2,?3,0,?4,?5,?6,'planned',?7,'{}',NULL)",params![step_id,run_id,decision_id,step.ordinal,step.actor_id,step.purpose,config_fingerprint]).map_err(|e|e.to_string())?;
    }
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

fn step_config_fingerprint(
    policy_json: &str,
    candidate: &crate::role_routing::selection::Candidate,
    step: &crate::role_routing::recipe::PlannedStep,
) -> Result<String, String> {
    let encoded = serde_json::to_vec(&json!({
        "policy": policy_json,
        "recipeId": &candidate.recipe_id,
        "actorIds": &candidate.actor_ids,
        "ordinal": step.ordinal,
        "purpose": step.purpose,
        "actorId": &step.actor_id,
    }))
    .map_err(|error| format!("Could not encode role-routing step fingerprint: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

/// Compiles the selected candidate into a finite plan and checks that the plan resolves exactly
/// the actors the candidate advertised. An invalid or unbounded recipe returns an error before any
/// root or step row is written, so dispatch stays at zero.
fn compile_selected_plan(
    policy: &crate::role_routing::RoleRoutingSettings,
    candidate: &crate::role_routing::selection::Candidate,
) -> Result<Vec<crate::role_routing::recipe::PlannedStep>, String> {
    let compiled = crate::role_routing::recipe::compile_recipe_by_id(policy, &candidate.recipe_id)?;
    let planned_actors = compiled
        .steps
        .iter()
        .map(|step| step.actor_id.as_str())
        .collect::<Vec<_>>();
    let candidate_actors = candidate
        .actor_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if planned_actors != candidate_actors {
        return Err("Role-routing compiled plan does not match the selected candidate".into());
    }
    Ok(compiled.steps)
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
    .filter(|candidate| candidate.exclusion_reason.is_none())
    .collect::<Vec<_>>();
    eligible.sort_by(|left, right| left.recipe_id.cmp(&right.recipe_id));
    let rules = eligible
        .first()
        .cloned()
        .ok_or_else(|| "No eligible role-routing response recipe is configured".to_string())?;
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
