use super::*;
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
pub(crate) fn apply_enabled_role_route(
    connection: &Connection,
    root_id: Option<&str>,
    route: &mut ConversationRouteSettings,
    reachability: &crate::providers::reachability::ReachabilitySnapshot,
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
    validate_actor_host(connection, actor, reachability)?;
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
pub(super) fn validate_actor_host(
    connection: &Connection,
    actor: &crate::role_routing::contracts::RoutingActor,
    reachability: &crate::providers::reachability::ReachabilitySnapshot,
) -> Result<(), String> {
    match actor.transport.as_str() {
        "provider" => {
            let provider_id = actor
                .provider_id
                .as_deref()
                .ok_or_else(|| "Role-routing provider actor has no provider id".to_string())?;
            if provider_id == crate::DYNAMIC_LAN_PROVIDER_ID
                && reachability.harness == crate::providers::reachability::Reachability::Unreachable
            {
                return Err("Role-routing LAN provider is unreachable before dispatch".into());
            }
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
pub(super) fn load_role_policy_for_root(
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
