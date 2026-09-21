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
    transaction.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,active_slot,origin,presentation_mode,started_at_ms,deadline_at_ms,scope_digest) VALUES(?1,?2,?3,?4,0,'queued',NULL,'text','visual',?5,NULL,'')",params![run_id,conversation_id,run_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_inputs(input_id,root_id,conversation_id,message_id,payload_digest,origin,disposition,received_at_ms) VALUES(?1,?2,?3,?4,'','text','accepted',?5)",params![format!("rr-input-{run_id}"),run_id,conversation_id,input_message_id,now_ms]).map_err(|e|e.to_string())?;
    transaction.execute("INSERT INTO rr_decisions(id,root_id,revision,input_id,features_json,candidates_json,selected_id,action,reason_codes_json,ranker_version,policy_id,created_at_ms) VALUES(?1,?2,0,?3,?4,?5,?6,'respond','[\"rules\"]','rules-v1',?7,?8)",params![decision_id,run_id,format!("rr-input-{run_id}"),feature_snapshot(&input_content,policy.limits.max_reasoning_steps).to_string(),candidates.to_string(),candidate.recipe_id,policy_id,now_ms]).map_err(|e|e.to_string())?;
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
    let compiled =
        crate::role_routing::recipe::compile_recipe_by_id(policy, &candidate.recipe_id)?;
    let planned_actors = compiled
        .steps
        .iter()
        .map(|step| step.actor_id.as_str())
        .collect::<Vec<_>>();
    if !candidate
        .actor_ids
        .iter()
        .all(|actor_id| planned_actors.contains(&actor_id.as_str()))
    {
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
    let root: Option<(String, i64)> = connection
        .query_row(
            "SELECT phase,cancel_requested FROM rr_roots WHERE root_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((phase, cancel_requested)) = root else {
        return Ok(());
    };
    // `accept_provider_turn` runs inside the assistant-message transaction first.  The outer
    // runtime finalizer still observes the provider terminal result afterwards, but must not
    // append a second terminal event or overwrite the already adopted root.
    if matches!(phase.as_str(), "completed" | "cancelled" | "failed") {
        return Ok(());
    }
    // A cancelled or barrier-held root is owned by the cancel / barrier path. The outer
    // finalizer must not turn a late provider result into the current answer.
    if cancel_requested != 0 {
        return Ok(());
    }
    if phase == "draining" {
        let step = active_step_or_ordinal_zero(connection, run_id)?;
        if let Some((step_id, _)) = step.as_ref() {
            let step_status = match status {
                "completed" => "succeeded",
                "cancelled" => "cancelled",
                _ => "failed",
            };
            let _ = crate::role_routing::steps::complete_step(
                connection,
                run_id,
                step_id,
                step_status,
                now_ms,
            );
        }
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
    let step = active_step_or_ordinal_zero(connection, run_id)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let step_revision = match step {
        Some((step_id, revision)) => {
            crate::role_routing::steps::complete_step(
                &transaction,
                run_id,
                &step_id,
                step_status,
                now_ms,
            )?;
            if let Some(message_id) = message_id {
                if status == "completed" {
                    crate::role_routing::steps::record_step_output(
                        &transaction,
                        run_id,
                        &step_id,
                        revision,
                        "answer",
                        &json!({ "messageId": message_id }).to_string(),
                        true,
                        now_ms,
                    )?;
                }
            }
            revision
        }
        None => 0,
    };
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
    match crate::role_routing::steps::active_reasoning_step(connection, run_id)? {
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
