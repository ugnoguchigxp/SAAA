//! Deterministic first-pass selection. A future ranker may only reorder these candidates.
use super::contracts::{RoleRoutingSettings, RoutingAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) recipe_id: String,
    pub(crate) actor_ids: Vec<String>,
    pub(crate) exclusion_reason: Option<String>,
}

pub(crate) fn candidates_for_action(
    settings: &RoleRoutingSettings,
    action: RoutingAction,
) -> Vec<Candidate> {
    settings
        .recipes
        .iter()
        .filter(|recipe| recipe.enabled && recipe.action == action)
        .map(|recipe| {
            let actor_ids = recipe
                .roles
                .iter()
                .filter_map(|role| actor_for_role(settings, role))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            Candidate {
                recipe_id: recipe.id.clone(),
                exclusion_reason: (actor_ids.len() != recipe.roles.len())
                    .then(|| "role_unavailable".into()),
                actor_ids,
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
    #[test]
    fn rules_choose_recipe_id_stably() {
        let mut s = RoleRoutingSettings::default();
        s.roles.reasoner = Some("qwen".into());
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
}
