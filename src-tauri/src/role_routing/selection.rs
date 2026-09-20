//! Deterministic first-pass selection. A future ranker may only reorder these candidates.
use super::contracts::{RoleRoutingSettings, RoutingAction};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) recipe_id: String,
    pub(crate) actor_ids: Vec<String>,
    pub(crate) exclusion_reason: Option<String>,
    pub(crate) reason_codes: Vec<String>,
}

/// Runtime facts supplied by the host before it chooses a recipe.  The selector is deliberately
/// pure: callers must collect prices and capability declarations before invoking it.
#[derive(Debug, Clone, Default)]
pub(crate) struct SelectionInput {
    pub(crate) cloud_allowed: bool,
    pub(crate) required_capabilities: HashSet<String>,
    /// A missing entry is an unknown price, which is ineligible whenever policy sets a budget.
    pub(crate) estimated_cost_micros: HashMap<String, Option<u64>>,
    pub(crate) sticky_actor_id: Option<String>,
}

pub(crate) fn candidates_for_action(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
) -> Vec<Candidate> {
    candidates_for_action_with_input(
        settings,
        action,
        &SelectionInput {
            cloud_allowed: true,
            ..SelectionInput::default()
        },
    )
}

pub(crate) fn candidates_for_action_with_input(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
    input: &SelectionInput,
) -> Vec<Candidate> {
    settings
        .recipes
        .iter()
        .filter(|recipe| recipe.enabled && recipe.action == action)
        .map(|recipe| {
            let actors = recipe
                .roles
                .iter()
                .filter_map(|role| actor_for_role(settings, role))
                .filter_map(|id| settings.actors.iter().find(|actor| actor.id == id))
                .collect::<Vec<_>>();
            let actor_ids = actors
                .iter()
                .map(|actor| actor.id.clone())
                .collect::<Vec<_>>();
            let mut reason_codes = Vec::new();
            if actor_ids.len() != recipe.roles.len() {
                reason_codes.push("role_unavailable".into());
            }
            if !input.cloud_allowed && actors.iter().any(|actor| actor.location == "cloud") {
                reason_codes.push("cloud_forbidden".into());
            }
            if actors.iter().any(|actor| {
                !input.required_capabilities.iter().all(|needed| {
                    actor
                        .capabilities
                        .iter()
                        .any(|capability| capability == needed)
                })
            }) {
                reason_codes.push("capability_missing".into());
            }
            if let Some(limit) = settings.limits.max_estimated_cost_micros {
                let costs = actors
                    .iter()
                    .map(|actor| {
                        input
                            .estimated_cost_micros
                            .get(&actor.id)
                            .copied()
                            .flatten()
                    })
                    .collect::<Option<Vec<_>>>();
                match costs.and_then(|costs| {
                    costs
                        .into_iter()
                        .try_fold(0_u64, |sum, cost| sum.checked_add(cost))
                }) {
                    Some(cost) if cost <= limit => {}
                    Some(_) => reason_codes.push("cost_budget_exceeded".into()),
                    None => reason_codes.push("cost_unknown".into()),
                }
            }
            Candidate {
                recipe_id: recipe.id.clone(),
                exclusion_reason: reason_codes.first().cloned(),
                actor_ids,
                reason_codes,
            }
        })
        .collect()
}
pub(crate) fn select_rule_candidate(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
) -> Option<Candidate> {
    candidates_for_action(settings, action)
        .into_iter()
        .filter(|candidate| candidate.exclusion_reason.is_none())
        .min_by(|a, b| a.recipe_id.cmp(&b.recipe_id))
}

pub(crate) fn select_rule_candidate_with_input(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
    input: &SelectionInput,
) -> Option<Candidate> {
    candidates_for_action_with_input(settings, action, input)
        .into_iter()
        .filter(|candidate| candidate.exclusion_reason.is_none())
        .min_by(|left, right| {
            let left_sticky = input
                .sticky_actor_id
                .as_ref()
                .is_some_and(|actor| left.actor_ids.iter().any(|id| id == actor));
            let right_sticky = input
                .sticky_actor_id
                .as_ref()
                .is_some_and(|actor| right.actor_ids.iter().any(|id| id == actor));
            right_sticky
                .cmp(&left_sticky)
                .then_with(|| left.recipe_id.cmp(&right.recipe_id))
        })
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
#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_routing::contracts::*;

    fn actor(id: &str, location: &str, capabilities: &[&str]) -> RoutingActor {
        RoutingActor {
            id: id.into(),
            label: id.into(),
            aliases: vec![],
            transport: "provider".into(),
            provider_id: Some(id.into()),
            model: None,
            location: location.into(),
            resource_group: "gpu".into(),
            max_input_bytes: 1024,
            capabilities: capabilities
                .iter()
                .map(|capability| (*capability).into())
                .collect(),
        }
    }
    #[test]
    fn rules_choose_recipe_id_stably() {
        let mut s = RoleRoutingSettings::default();
        s.roles.reasoner = Some("qwen".into());
        s.actors = vec![actor("qwen", "local", &["reason"])];
        s.recipes = vec![
            RoutingRecipe {
                id: "zeta".into(),
                action: RoutingAction::Respond,
                roles: vec!["reasoner".into()],
                enabled: true,
            },
            RoutingRecipe {
                id: "alpha".into(),
                action: RoutingAction::Respond,
                roles: vec!["reasoner".into()],
                enabled: true,
            },
        ];
        assert_eq!(
            select_rule_candidate(&s, RoutingAction::Respond)
                .expect("candidate")
                .recipe_id,
            "alpha"
        );
    }

    #[test]
    fn rr_06_cloud_filter_and_unknown_cost_are_recorded() {
        let mut settings = RoleRoutingSettings::default();
        settings.roles.reasoner = Some("cloud".into());
        settings.actors = vec![actor("cloud", "cloud", &["reason"])];
        settings.recipes = vec![RoutingRecipe {
            id: "respond".into(),
            action: RoutingAction::Respond,
            roles: vec!["reasoner".into()],
            enabled: true,
        }];
        settings.limits.max_estimated_cost_micros = Some(10);
        let candidates = candidates_for_action_with_input(
            &settings,
            RoutingAction::Respond,
            &SelectionInput {
                cloud_allowed: false,
                ..SelectionInput::default()
            },
        );
        assert_eq!(
            candidates[0].reason_codes,
            vec!["cloud_forbidden", "cost_unknown"]
        );
    }

    #[test]
    fn rr_06_sticky_actor_wins_the_deterministic_tie_break() {
        let mut settings = RoleRoutingSettings::default();
        settings.roles.reasoner = Some("a".into());
        settings.roles.advanced = Some("b".into());
        settings.actors = vec![
            actor("a", "local", &["reason"]),
            actor("b", "local", &["reason"]),
        ];
        settings.recipes = vec![
            RoutingRecipe {
                id: "alpha".into(),
                action: RoutingAction::Respond,
                roles: vec!["reasoner".into()],
                enabled: true,
            },
            RoutingRecipe {
                id: "zeta".into(),
                action: RoutingAction::Respond,
                roles: vec!["advanced".into()],
                enabled: true,
            },
        ];
        assert_eq!(
            select_rule_candidate_with_input(
                &settings,
                RoutingAction::Respond,
                &SelectionInput {
                    cloud_allowed: true,
                    sticky_actor_id: Some("b".into()),
                    ..SelectionInput::default()
                }
            )
            .expect("candidate")
            .recipe_id,
            "zeta"
        );
    }
}
