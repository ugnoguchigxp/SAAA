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

/// Commits a non-final provider candidate and claims the next planned step atomically. The draft
/// body remains process-local; the routing ledger stores only a digest and byte count. Returns
/// `false` without mutation when the active step is the final step, leaving final adoption to the
/// assistant-message transaction.
pub(crate) fn advance_provider_step(
    connection: &Connection,
    run_id: &str,
    content: &str,
    now_ms: i64,
) -> Result<bool, String> {
    advance_provider_step_with_usage(connection, run_id, content, None, now_ms)
}

pub(crate) fn advance_provider_step_with_usage(
    connection: &Connection,
    run_id: &str,
    content: &str,
    usage_json: Option<&str>,
    now_ms: i64,
) -> Result<bool, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let root: Option<(i64, i64, String)> = transaction
        .query_row(
            "SELECT revision,cancel_requested,phase FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, cancel_requested, phase)) = root else {
        return Ok(false);
    };
    if phase != "responding" || cancel_requested != 0 {
        return Err("Role-routing intermediate result is not adoptable".into());
    }
    let active_step: Option<(String, i64)> = transaction
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((step_id, step_revision)) = active_step else {
        return Err("Role-routing intermediate result has no active step".into());
    };
    if step_revision != revision {
        return Err("Role-routing intermediate result is stale".into());
    }
    let has_next: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_steps current JOIN rr_steps next ON next.root_id=current.root_id AND next.revision=current.revision AND next.ordinal>current.ordinal AND next.status='planned' WHERE current.id=?1 AND current.root_id=?2)",
            params![step_id, run_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_next {
        return Ok(false);
    }
    if let Some(usage_json) = usage_json {
        record_step_usage(&transaction, run_id, usage_json)?;
    }
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing intermediate step was already completed".into());
    }
    let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
    crate::role_routing::steps::record_step_output(
        &transaction,
        run_id,
        &step_id,
        revision,
        "intermediate",
        &json!({"sha256": digest, "bytes": content.len()}).to_string(),
        false,
        now_ms,
    )?;
    crate::role_routing::steps::claim_next_planned_step(&transaction, run_id, revision, now_ms)?
        .ok_or_else(|| "Role-routing next step disappeared before claim".to_string())?;
    append_event(&transaction, run_id, "step_completed", now_ms)?;
    append_event(&transaction, run_id, "step_started", now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(true)
}

#[derive(Debug)]
pub(crate) enum ReviewStepOutcome {
    Revise(crate::role_routing::revision::ReviewRevisionDecision),
    AwaitPremium(crate::role_routing::proposals::ProposalReceipt),
    KeepDraft,
}

/// Persists a typed review and its host decision, then atomically claims or cancels revision.
pub(crate) fn advance_review_step(
    connection: &Connection,
    run_id: &str,
    response: &crate::role_routing::review::ReviewResponse,
    usage_json: Option<&str>,
    now_ms: i64,
) -> Result<ReviewStepOutcome, String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let (revision, cancel_requested, phase, policy_id, policy_json, root_deadline): (
        i64,
        i64,
        String,
        String,
        String,
        Option<i64>,
    ) = transaction
        .query_row(
            "SELECT r.revision,r.cancel_requested,r.phase,r.policy_id,p.config_json,r.deadline_at_ms FROM rr_roots r JOIN rr_policy_versions p ON p.id=r.policy_id WHERE r.root_id=?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(|_| "Role-routing review root is unavailable".to_string())?;
    if phase != "responding" || cancel_requested != 0 {
        return Err("Role-routing review is not adoptable".into());
    }
    let (step_id, step_revision): (String, i64) = transaction
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' AND purpose='review'",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "Role-routing review has no active step".to_string())?;
    if step_revision != revision {
        return Err("Role-routing review is stale".into());
    }
    if let Some(usage_json) = usage_json {
        transaction
            .execute(
                "UPDATE rr_steps SET usage_json=?1 WHERE id=?2 AND root_id=?3 AND revision=?4 AND status='running'",
                params![usage_json, step_id, run_id, revision],
            )
            .map_err(|error| error.to_string())?;
    }
    crate::role_routing::repository::record_review_response(
        &transaction,
        &step_id,
        response,
        now_ms,
    )?;
    let review_output_id: String = transaction
        .query_row(
            "SELECT id FROM rr_outputs WHERE step_id=?1 AND revision=?2 AND kind='review' AND accepted=1",
            params![step_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
        .map_err(|_| "Stored role-routing policy is invalid".to_string())?;
    let decision = crate::role_routing::repository::record_review_revision_decision(
        &transaction,
        &review_output_id,
        policy.limits.max_review_rounds,
        now_ms,
    )?;
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing review step was already completed".into());
    }
    let outcome = if decision.revision_allowed {
        crate::role_routing::steps::claim_next_planned_step(
            &transaction,
            run_id,
            revision,
            now_ms,
        )?
        .ok_or_else(|| "Role-routing revise step disappeared before claim".to_string())?;
        append_event(&transaction, run_id, "step_started", now_ms)?;
        ReviewStepOutcome::Revise(decision)
    } else {
        crate::role_routing::steps::settle_unfinished_steps(
            &transaction,
            run_id,
            revision,
            "cancelled",
            now_ms,
        )?;
        let premium = policy
            .roles
            .premium
            .as_deref()
            .filter(|_| policy.premium_approval == "per_request")
            .filter(|_| !decision.unresolved_issues.is_empty())
            // A metered cloud step with no trusted quote must not be proposed under a cost cap.
            .filter(|_| policy.limits.max_estimated_cost_micros.is_none());
        if let Some(candidate_id) = premium {
            let expires_at_ms = root_deadline
                .unwrap_or_else(|| now_ms.saturating_add(60_000))
                .min(now_ms.saturating_add(60_000));
            if expires_at_ms > now_ms {
                let receipt = crate::role_routing::proposals::record(
                    &transaction,
                    &crate::role_routing::proposals::Proposal {
                        id: format!("rr-proposal-{run_id}-{revision}"),
                        root_id: run_id.to_string(),
                        candidate_id: candidate_id.to_string(),
                        policy_id,
                        revision: u32::try_from(revision)
                            .map_err(|_| "Role-routing revision is invalid".to_string())?,
                        expires_at_ms,
                    },
                    None,
                    now_ms,
                )?;
                append_event(&transaction, run_id, "premium_proposed", now_ms)?;
                ReviewStepOutcome::AwaitPremium(receipt)
            } else {
                ReviewStepOutcome::KeepDraft
            }
        } else {
            ReviewStepOutcome::KeepDraft
        }
    };
    append_event(&transaction, run_id, "review_completed", now_ms)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(outcome)
}

/// Adopts the original process-local draft after review found no verified reason to revise.
pub(crate) fn accept_reviewed_draft(
    connection: &Connection,
    run_id: &str,
    draft_step_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let (revision, phase, cancelled): (i64, String, i64) = connection
        .query_row(
            "SELECT revision,phase,cancel_requested FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| "Role-routing reviewed root is unavailable".to_string())?;
    if phase != "responding" || cancelled != 0 {
        return Err("Role-routing reviewed draft is not adoptable".into());
    }
    let eligible: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_steps d WHERE d.id=?1 AND d.root_id=?2 AND d.revision=?3 AND d.status='succeeded' AND d.purpose IN ('respond','reconsider') AND EXISTS(SELECT 1 FROM rr_steps v WHERE v.root_id=d.root_id AND v.revision=d.revision AND v.purpose='review' AND v.status='succeeded') AND NOT EXISTS(SELECT 1 FROM rr_steps x WHERE x.root_id=d.root_id AND x.revision=d.revision AND x.status IN ('planned','running','draining')))",
            params![draft_step_id, run_id, revision],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !eligible {
        return Err("Role-routing reviewed draft receipt is incomplete".into());
    }
    if !crate::role_routing::steps::finalize_root(connection, run_id, revision, message_id, now_ms)?
    {
        return Err("Role-routing reviewed draft was already accepted".into());
    }
    connection
        .execute(
            "INSERT INTO rr_outputs(id,step_id,revision,kind,payload_json,accepted,created_at_ms) VALUES(?1,?2,?3,'answer',?4,1,?5)",
            params![format!("rr-final-{draft_step_id}"), draft_step_id, revision, json!({"messageId": message_id}).to_string(), now_ms],
        )
        .map_err(|error| error.to_string())?;
    append_event(connection, run_id, "answer_committed", now_ms)?;
    record_provider_outcome(connection, run_id, true, message_id, revision, now_ms)?;
    crate::role_routing::learning::repository::mark_root_dirty(connection, run_id)?;
    Ok(())
}

pub(crate) fn record_provider_turn_finish(
    connection: &Connection,
    run_id: &str,
    status: &str,
    message_id: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let root: Option<(String, i64, i64)> = transaction
        .query_row(
            "SELECT phase,cancel_requested,revision FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((current_phase, cancel_requested, root_revision)) = root else {
        return Ok(());
    };
    // `accept_provider_turn` runs inside the assistant-message transaction first.  The outer
    // runtime finalizer still observes the provider terminal result afterwards, but must not
    // append a second terminal event or overwrite the already adopted root.
    if matches!(current_phase.as_str(), "completed" | "cancelled" | "failed") {
        return Ok(());
    }
    // A cancelled or barrier-held root is owned by the cancel / barrier path. The outer
    // finalizer must not turn a late provider result into the current answer.
    if cancel_requested != 0 {
        return Ok(());
    }
    if current_phase == "draining" {
        let step = active_step_or_ordinal_zero(&transaction, run_id)?;
        if let Some((step_id, _)) = step.as_ref() {
            let step_status = match status {
                "completed" => "succeeded",
                "cancelled" => "cancelled",
                _ => "failed",
            };
            crate::role_routing::steps::complete_step(
                &transaction,
                run_id,
                step_id,
                step_status,
                now_ms,
            )?;
        }
        return transaction.commit().map_err(|error| error.to_string());
    }
    if current_phase != "responding" {
        return Err("Role-routing root has an invalid phase at provider completion".into());
    }
    let step_status = match status {
        "completed" => "succeeded",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let terminal_phase = match status {
        "completed" => "completed",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    let Some((step_id, step_revision)) = active_step_or_ordinal_zero(&transaction, run_id)? else {
        return Err("Role-routing root has no step to complete".into());
    };
    if step_revision != root_revision {
        return Err("Role-routing provider result belongs to a stale revision".into());
    }
    if !crate::role_routing::steps::complete_step(
        &transaction,
        run_id,
        &step_id,
        step_status,
        now_ms,
    )? {
        return Err("Role-routing provider step was already completed".into());
    }
    if status != "completed" {
        let remaining_status = if status == "cancelled" {
            "cancelled"
        } else {
            "interrupted"
        };
        crate::role_routing::steps::settle_unfinished_steps(
            &transaction,
            run_id,
            root_revision,
            remaining_status,
            now_ms,
        )?;
    }
    if status == "completed" {
        let message_id =
            message_id.ok_or_else(|| "Completed role-routing result has no message".to_string())?;
        crate::role_routing::steps::record_step_output(
            &transaction,
            run_id,
            &step_id,
            step_revision,
            "answer",
            &json!({ "messageId": message_id }).to_string(),
            true,
            now_ms,
        )?;
    }
    let changed = transaction
        .execute(
            "UPDATE rr_roots SET phase=?1, result_message_id=?2, active_slot=NULL WHERE root_id=?3 AND phase='responding' AND revision=?4 AND cancel_requested=0",
            params![terminal_phase, message_id, run_id, root_revision],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing root changed before provider completion committed".into());
    }
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
        step_revision,
        now_ms,
    )?;
    crate::role_routing::learning::repository::mark_root_dirty(&transaction, run_id)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// Prefers the active step, falling back to ordinal 0 for legacy rows created before the step
/// ledger selected the active step.
fn active_step_or_ordinal_zero(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<(String, i64)>, String> {
    let running = connection
        .query_row(
            "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status IN ('running','draining') ORDER BY ordinal LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match running {
        Some(active) => Ok(Some(active)),
        None => connection
            .query_row(
                "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND ordinal=0",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string()),
    }
}

/// Outcome of storing an input receipt. A retry with the same payload is a duplicate that must
/// return the original receipt without creating new side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputReceiptDisposition {
    Accepted,
    Duplicate,
}

/// Stores or restores an `rr_inputs` receipt keyed by `(conversationId, inputId)`. The same input
/// with the same payload digest is idempotent; the same input with a different digest is a
/// conflict and must not create a second row or run a second dispatch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_input_receipt(
    connection: &Connection,
    root_id: Option<&str>,
    input_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload_digest: &str,
    source_id: Option<&str>,
    origin: &str,
    disposition: &str,
    generation: i64,
    now_ms: i64,
) -> Result<InputReceiptDisposition, String> {
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT payload_digest,message_id FROM rr_inputs WHERE conversation_id=?1 AND input_id=?2",
            params![conversation_id, input_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((existing_digest, _existing_message)) = existing {
        if existing_digest == payload_digest {
            return Ok(InputReceiptDisposition::Duplicate);
        }
        return Err("Role-routing input receipt conflicts with a different payload".into());
    }
    // A retransmitted capture (for example a repeated ASR final) shares its source id but may
    // arrive with a fresh input id. Only the first receipt per source is accepted.
    if let Some(source_id) = source_id {
        let existing_source: Option<(String, String)> = connection
            .query_row(
                "SELECT input_id,payload_digest FROM rr_inputs WHERE conversation_id=?1 AND source_id=?2",
                params![conversation_id, source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((existing_input, existing_digest)) = existing_source {
            if existing_input != input_id && existing_digest == payload_digest {
                return Ok(InputReceiptDisposition::Duplicate);
            }
            if existing_input != input_id && existing_digest != payload_digest {
                return Err("Role-routing input source conflicts with a different payload".into());
            }
        }
    }
    connection
        .execute(
            "INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,source_id,disposition,generation,received_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                input_id,
                root_id,
                conversation_id,
                message_id,
                payload_digest,
                origin,
                source_id,
                disposition,
                generation,
                now_ms
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(InputReceiptDisposition::Accepted)
}

/// Records an input that arrived while a root was active and raises the durable input barrier in
/// the same transaction. The stored generation is the root revision the classifier result must
/// match before an amendment may advance the revision.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_active_input_barrier(
    connection: &Connection,
    root_id: &str,
    input_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload_digest: &str,
    source_id: Option<&str>,
    origin: &str,
    now_ms: i64,
) -> Result<InputReceiptDisposition, String> {
    let root: Option<(String, i64)> = connection
        .query_row(
            "SELECT phase,revision FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((phase, revision)) = root else {
        return Err("Role-routing barrier references an unknown root".into());
    };
    if !matches!(phase.as_str(), "queued" | "responding" | "draining") {
        return Err("Role-routing barrier requires an active root".into());
    }
    let disposition = record_input_receipt(
        connection,
        Some(root_id),
        input_id,
        conversation_id,
        message_id,
        payload_digest,
        source_id,
        origin,
        "accepted",
        revision,
        now_ms,
    )?;
    if disposition == InputReceiptDisposition::Duplicate {
        return Ok(disposition);
    }
    crate::role_routing::coordinator::apply_in_transaction(
        connection,
        root_id,
        crate::role_routing::reducer::Event::InputBarrier,
        now_ms,
    )?;
    Ok(disposition)
}

/// A classifier result may only act on the generation it was produced for. A late classifier for
/// an older revision must not amend the current one.
pub(crate) fn classifier_generation_matches(
    connection: &Connection,
    input_id: &str,
    conversation_id: &str,
    current_revision: i64,
) -> Result<bool, String> {
    let generation: Option<i64> = connection
        .query_row(
            "SELECT generation FROM rr_inputs WHERE conversation_id=?1 AND input_id=?2",
            params![conversation_id, input_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(generation.is_some_and(|generation| generation == current_revision))
}

/// Counts inputs still waiting on classification for a root. Used to assert that multiple
/// pending inputs are not dropped when one is resolved.
pub(crate) fn pending_input_count(connection: &Connection, root_id: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT count(*) FROM rr_inputs WHERE root_id=?1",
            [root_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

/// Cancels every queued or active routing root when the feature is disabled. The database fence
/// commits with the settings change; process-local cancellation is signalled by the command layer
/// after this transaction returns.
pub(crate) fn cancel_all_for_disable(connection: &Connection, now_ms: i64) -> Result<(), String> {
    let root_ids = connection
        .prepare(
            "SELECT root_id FROM rr_roots
             WHERE phase IN ('queued','responding','draining')
             ORDER BY started_at_ms,root_id",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for root_id in root_ids {
        crate::role_routing::coordinator::apply_in_transaction(
            connection,
            &root_id,
            crate::role_routing::reducer::Event::Cancel,
            now_ms,
        )?;
        connection
            .execute(
                "UPDATE rr_steps SET status='cancelled',cancel_requested=1,completed_at_ms=?1,error_code='routing-disabled'
                 WHERE root_id=?2 AND status IN ('planned','running','draining')",
                params![now_ms, root_id],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute(
            "UPDATE rr_speech SET status='cancelled',updated_at_ms=?1
             WHERE status IN ('queued','playing')",
            [now_ms],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE rr_premium_proposals SET status='expired'
             WHERE status IN ('proposed','approved') AND consumed_at_ms IS NULL",
            [],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// While routing is disabled, legacy execution stays fenced until every cancelled runtime child
/// and role-owned tool operation is terminal. This avoids overlapping old side effects with a new
/// legacy answer.
pub(crate) fn disable_drain_in_progress(connection: &Connection) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM rr_roots r
               JOIN runtime_runs run ON run.id=r.runtime_run_id
               WHERE r.cancel_requested=1 AND run.status IN ('running','queued')
               UNION ALL
               SELECT 1 FROM rr_tool_links l
               JOIN rr_roots r ON r.root_id=l.root_id
               WHERE r.cancel_requested=1 AND l.dispatch_state IN ('reserved','dispatched')
               UNION ALL
               SELECT 1 FROM rr_speech s
               JOIN rr_roots r ON r.root_id=s.root_id
               WHERE r.cancel_requested=1 AND s.status IN ('queued','playing')
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

pub(crate) fn disabled_runtime_run_ids(connection: &Connection) -> Result<Vec<String>, String> {
    connection
        .prepare(
            "SELECT DISTINCT runtime_run_id FROM rr_roots
             WHERE cancel_requested=1 AND runtime_run_id IS NOT NULL",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Stores coarse actor progress without retaining any partial provider text.
pub(crate) fn record_actor_activity(
    connection: &Connection,
    root_id: &str,
    kind: &str,
    now_ms: i64,
) -> Result<bool, String> {
    if !matches!(kind, "provider_started" | "provider_progress") {
        return Err("Unsupported role-routing activity kind".into());
    }
    let active = connection
        .query_row(
            "SELECT phase IN ('queued','responding','draining') FROM rr_roots WHERE root_id=?1",
            [root_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if !active {
        return Ok(false);
    }
    append_event(connection, root_id, "activity", now_ms)?;
    Ok(true)
}

/// Atomically adopts a normal provider result when the role-routing root is still current.
/// It is invoked from the assistant-message transaction after the message and scope link exist.
pub(crate) fn accept_provider_turn(
    connection: &Connection,
    run_id: &str,
    message_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let root: Option<(i64, i64, String)> = connection
        .query_row(
            "SELECT revision,cancel_requested,phase FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((revision, cancelled, phase)) = root else {
        return Ok(());
    };
    if cancelled != 0 {
        return Err("Role-routing result was cancelled".into());
    }
    // Only the active `responding` phase may adopt a final answer. `draining` means an input
    // barrier or a stop request is up, and terminal phases are already settled; a result that
    // arrives then is held or dropped by the caller, never recorded as the current answer.
    if phase != "responding" {
        return Err(format!(
            "Role-routing result is not adoptable from phase {phase}"
        ));
    }
    // The expected revision comes from the result side step row, not from the root's current
    // revision. A late result produced for an older revision must not be adopted against the
    // current one even if the root has since advanced. The active step is selected by the step
    // ledger, so ordinal 0 is not hard-coded.
    let Some((step_id, step_revision)) =
        crate::role_routing::steps::active_reasoning_step(connection, run_id)?
    else {
        return Err("Role-routing result has no active step".into());
    };
    if step_revision != revision {
        return Err("Role-routing result is stale for the active step".into());
    }
    if !crate::role_routing::steps::complete_step(
        connection,
        run_id,
        &step_id,
        "succeeded",
        now_ms,
    )? {
        return Err("Role-routing result step already completed".into());
    }
    if !crate::role_routing::steps::finalize_root(connection, run_id, revision, message_id, now_ms)?
    {
        return Err("Role-routing result is stale or already accepted".into());
    }
    crate::role_routing::steps::record_step_output(
        connection,
        run_id,
        &step_id,
        revision,
        "answer",
        &json!({ "messageId": message_id }).to_string(),
        true,
        now_ms,
    )?;
    append_event(connection, run_id, "answer_committed", now_ms)?;
    record_provider_outcome(connection, run_id, true, message_id, revision, now_ms)?;
    crate::role_routing::learning::repository::mark_root_dirty(connection, run_id)?;
    Ok(())
}

/// Stores SDK-reported usage in the same transaction that adopts the final answer. The value is
/// produced by the typed sidecar protocol and checked again by SQLite's JSON constraint.
pub(crate) fn record_step_usage(
    connection: &Connection,
    run_id: &str,
    usage_json: &str,
) -> Result<(), String> {
    let Some((step_id, _revision)) =
        crate::role_routing::steps::active_reasoning_step(connection, run_id)?
    else {
        return Err("Role-routing usage is stale or has no active step".into());
    };
    let changed = connection
        .execute(
            "UPDATE rr_steps SET usage_json=?1 WHERE id=?2 AND root_id=?3 AND status IN ('planned','running','draining')",
            params![usage_json, step_id, run_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Role-routing usage is stale or has no active step".into());
    }
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

    #[test]
    fn rr_04_queue_full_leaves_no_input_message() {
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
                    &tx, "run", "c", "text", None, "visual", 2,
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
    fn ai_08_provider_terminal_result_is_recorded_for_the_dispatch_decision() {
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

    #[test]
    fn rr_12_old_revision_result_rejected() {
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
    fn rr_16_pending_input_blocks_real_acceptance() {
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
    fn rr_12_db_failure_no_speech() {
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
    fn rr_04_receipt_retry_and_conflict() {
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
    fn rr_14_asr_duplicate_receipt() {
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
    fn rr_16_multiple_pending_inputs() {
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
    fn rr_22_usage_is_saved_with_the_active_step() {
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
}
