//! Finite recipe compiler for role routing.
//!
//! A recipe is compiled from a known template (the action) into a bounded, acyclic list of steps.
//! Arbitrary graphs, recursion, scripts, and URL execution are not representable. The compiler
//! validates ordering, dependencies, the step budget, and reviewer independence before any
//! dispatch, so an invalid recipe produces zero actor starts.
#![allow(dead_code)]

use super::contracts::{RoleRoutingSettings, RoutingAction, RoutingLimits};

/// One planned step in a compiled recipe. `actor_id` is resolved at compile time; `depends_on`
/// references earlier ordinals only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedStep {
    pub(crate) ordinal: u32,
    pub(crate) purpose: &'static str,
    pub(crate) role: String,
    pub(crate) actor_id: String,
    pub(crate) depends_on: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompiledRecipe {
    pub(crate) recipe_id: String,
    pub(crate) action: RoutingAction,
    pub(crate) steps: Vec<PlannedStep>,
}

/// The fixed purpose sequence for each supported action. The list length fixes how many roles the
/// recipe must resolve; an action with no execution (Finalize/Cancel) yields no steps.
fn step_purposes(action: &RoutingAction) -> &'static [&'static str] {
    match action {
        RoutingAction::Respond | RoutingAction::Explain => &["respond"],
        RoutingAction::Clarify => &["frontend"],
        RoutingAction::ReconsiderSame | RoutingAction::ReconsiderOther => &["reconsider"],
        RoutingAction::ReviewOther => &["review", "revise"],
        RoutingAction::Revise => &["revise"],
        RoutingAction::ProposeUpgrade => &["propose"],
        RoutingAction::Finalize | RoutingAction::Cancel => &[],
    }
}

fn purpose_depends_on(purposes: &[&'static str], ordinal: usize) -> Vec<u32> {
    // Only the revise step consumes another step's output; every other template is linear.
    if ordinal > 0 && purposes.get(ordinal) == Some(&"revise") {
        vec![(ordinal - 1) as u32]
    } else {
        Vec::new()
    }
}

/// Validates a compiled plan against the root budget. Dependencies must reference strictly earlier
/// ordinals and ordinals must be contiguous, which makes cycles impossible by construction.
pub(crate) fn validate_plan(plan: &[PlannedStep], limits: &RoutingLimits) -> Result<(), String> {
    if plan.len() > usize::from(limits.max_reasoning_steps) {
        return Err("Role-routing recipe exceeds the reasoning step budget".into());
    }
    for (index, step) in plan.iter().enumerate() {
        if step.ordinal as usize != index {
            return Err("Role-routing recipe ordinals must be contiguous from zero".into());
        }
        for dependency in &step.depends_on {
            if *dependency as usize >= index {
                return Err("Role-routing recipe dependency must reference an earlier step".into());
            }
        }
    }
    Ok(())
}

/// Two actors are the same underlying deployment only when transport, provider, and model all
/// match. A different actor ID is not evidence of an independent reviewer.
fn actor_fingerprint(actor: &super::contracts::RoutingActor) -> String {
    format!(
        "{}:{}:{}",
        actor.transport,
        actor.provider_id.as_deref().unwrap_or(""),
        actor.model.as_deref().unwrap_or("")
    )
}

fn actor_for_role<'a>(settings: &'a RoleRoutingSettings, role: &str) -> Option<&'a str> {
    match role {
        "frontend" => settings.roles.frontend.as_deref(),
        "reasoner" => settings.roles.reasoner.as_deref(),
        "advanced" => settings.roles.advanced.as_deref(),
        "reviewer" => settings.roles.reviewer.as_deref(),
        "premium" => settings.roles.premium.as_deref(),
        "tool_specialist" => settings.roles.tool_specialist.as_deref(),
        _ => None,
    }
}

/// Compiles the deterministic rules candidate for `action` into a finite plan. Returns an error
/// (and therefore zero dispatch) for unknown primitives, unbounded recipes, unresolved roles, or a
/// reviewer whose deployment fingerprint matches the author's.
pub(crate) fn compile_recipe(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
) -> Result<CompiledRecipe, String> {
    let candidate = super::selection::select_rule_candidate(settings, action.clone())
        .ok_or_else(|| format!("No eligible role-routing recipe for action {action:?}"))?;
    let compiled = compile_recipe_by_id(settings, &candidate.recipe_id)?;
    if compiled.action != action {
        return Err("Role-routing recipe action does not match the requested action".into());
    }
    Ok(compiled)
}

/// Compiles a specific recipe id. The receipt selects an adaptive candidate by recipe id, so the
/// compiler must be able to compile that exact id without re-running selection.
pub(crate) fn compile_recipe_by_id(
    settings: &RoleRoutingSettings,
    recipe_id: &str,
) -> Result<CompiledRecipe, String> {
    let recipe = settings
        .recipes
        .iter()
        .find(|recipe| recipe.id == recipe_id)
        .ok_or_else(|| "Role-routing recipe is not configured".to_string())?;
    if !recipe.enabled {
        return Err("Role-routing recipe is disabled".into());
    }
    let action = recipe.action.clone();
    let purposes = step_purposes(&action);
    if purposes.len() != recipe.roles.len() {
        return Err("Role-routing recipe roles do not match its template".into());
    }
    let mut steps = Vec::with_capacity(purposes.len());
    for (index, (purpose, role)) in purposes.iter().zip(recipe.roles.iter()).enumerate() {
        let actor_id = actor_for_role(settings, role)
            .ok_or_else(|| format!("Role-routing role {role} is not configured"))?
            .to_string();
        steps.push(PlannedStep {
            ordinal: index as u32,
            purpose,
            role: role.clone(),
            actor_id,
            depends_on: purpose_depends_on(purposes, index),
        });
    }
    // The reviewer of a `review_other` recipe must be a different deployment from the author.
    if action == RoutingAction::ReviewOther {
        let reviewer = steps
            .iter()
            .find(|step| step.purpose == "review")
            .and_then(|step| {
                settings
                    .actors
                    .iter()
                    .find(|actor| actor.id == step.actor_id)
            });
        let author = steps
            .iter()
            .find(|step| step.purpose == "revise")
            .and_then(|step| {
                settings
                    .actors
                    .iter()
                    .find(|actor| actor.id == step.actor_id)
            });
        match (reviewer, author) {
            (Some(reviewer), Some(author))
                if reviewer.id != author.id
                    && actor_fingerprint(reviewer) != actor_fingerprint(author) => {}
            _ => {
                return Err(
                    "Role-routing review requires an independent reviewer deployment".into(),
                )
            }
        }
    }
    validate_plan(&steps, &settings.limits)?;
    Ok(CompiledRecipe {
        recipe_id: recipe.id.clone(),
        action,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_routing::contracts::{RoutingActor, RoutingRecipe};

    fn actor(id: &str, model: &str, location: &str) -> RoutingActor {
        RoutingActor {
            id: id.into(),
            label: id.into(),
            aliases: vec![],
            transport: "codex_sdk".into(),
            provider_id: None,
            model: Some(model.into()),
            location: location.into(),
            resource_group: "gpu".into(),
            max_input_bytes: 1024,
            capabilities: vec!["reason".into()],
        }
    }

    fn review_settings() -> RoleRoutingSettings {
        let mut settings = RoleRoutingSettings::default();
        settings.enabled = true;
        settings.actors = vec![
            actor("qwen", "qwen-model", "local"),
            actor("sol", "sol-model", "cloud"),
        ];
        settings.roles.reasoner = Some("qwen".into());
        settings.roles.advanced = Some("sol".into());
        settings.roles.reviewer = Some("qwen".into());
        settings.recipes = vec![RoutingRecipe {
            id: "cross".into(),
            action: RoutingAction::ReviewOther,
            roles: vec!["reviewer".into(), "advanced".into()],
            enabled: true,
        }];
        settings
    }

    #[test]
    fn rr_06_recipe_invalid_dependency() {
        let limits = RoutingLimits::default();
        let bad = vec![
            PlannedStep {
                ordinal: 0,
                purpose: "respond",
                role: "reasoner".into(),
                actor_id: "qwen".into(),
                depends_on: vec![1],
            },
            PlannedStep {
                ordinal: 1,
                purpose: "revise",
                role: "reasoner".into(),
                actor_id: "qwen".into(),
                depends_on: vec![],
            },
        ];
        assert!(validate_plan(&bad, &limits).is_err());
        // A dependency on a missing/forward ordinal is rejected even with contiguous ordinals.
        let mut forward = bad.clone();
        forward[1].depends_on = vec![0];
        forward[0].depends_on = vec![5];
        assert!(validate_plan(&forward, &limits).is_err());
    }

    #[test]
    fn rr_22_recipe_all_branches_bounded() {
        let mut settings = review_settings();
        settings.limits.max_reasoning_steps = 1;
        // The review template compiles to two steps, which exceeds a one-step budget.
        assert!(compile_recipe(&settings, RoutingAction::ReviewOther).is_err());
        settings.limits.max_reasoning_steps = 2;
        let compiled =
            compile_recipe(&settings, RoutingAction::ReviewOther).expect("compiled review recipe");
        assert_eq!(compiled.steps.len(), 2);
        assert_eq!(compiled.steps[1].depends_on, vec![0]);
    }

    #[test]
    fn rr_06_self_review_alias_rejected() {
        let mut settings = review_settings();
        // A second actor that is a different ID but the same deployment as the reviewer.
        settings.actors = vec![
            actor("qwen", "qwen-model", "local"),
            actor("qwen-mirror", "qwen-model", "local"),
        ];
        settings.roles.advanced = Some("qwen-mirror".into());
        settings.roles.reviewer = Some("qwen".into());
        assert!(
            compile_recipe(&settings, RoutingAction::ReviewOther).is_err(),
            "an aliased deployment of the same model must not count as an independent review"
        );
    }
}
