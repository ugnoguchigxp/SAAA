//! Maps an in-memory reachability snapshot onto actor ids. No database or network I/O.
use super::contracts::RoleRoutingSettings;
use super::selection::SelectionInput;
use crate::providers::reachability::{Reachability, ReachabilitySnapshot};
use crate::DYNAMIC_LAN_PROVIDER_ID;
use std::collections::HashSet;

/// AppState の観測を、policy 上の actor id 集合に写す。DB も I/O も触らない。
pub(crate) fn selection_input_for(
    policy: &RoleRoutingSettings,
    snapshot: &ReachabilitySnapshot,
) -> SelectionInput {
    let mut unreachable_actor_ids = HashSet::new();
    if snapshot.harness == Reachability::Unreachable {
        for actor in &policy.actors {
            if actor.transport == "provider"
                && actor.provider_id.as_deref() == Some(DYNAMIC_LAN_PROVIDER_ID)
            {
                unreachable_actor_ids.insert(actor.id.clone());
            }
        }
    }
    SelectionInput {
        cloud_allowed: true,
        unreachable_actor_ids,
        ..SelectionInput::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_routing::contracts::RoutingActor;

    fn actor(id: &str, provider_id: &str) -> RoutingActor {
        RoutingActor {
            id: id.into(),
            label: id.into(),
            aliases: vec![],
            transport: "provider".into(),
            provider_id: Some(provider_id.into()),
            model: None,
            location: "local".into(),
            resource_group: "lan".into(),
            max_input_bytes: 1024,
            larm_provider: None,
            capabilities: vec!["reason".into()],
        }
    }

    #[test]
    fn rr_ls_12_unreachable_marks_only_the_dynamic_lan_actor() {
        let mut policy = RoleRoutingSettings::default();
        policy.actors = vec![
            actor("larm", DYNAMIC_LAN_PROVIDER_ID),
            actor("other-local", "other-provider"),
            RoutingActor {
                id: "cloud".into(),
                label: "Cloud".into(),
                aliases: vec![],
                transport: "provider".into(),
                provider_id: Some("cloud-api".into()),
                model: None,
                location: "cloud".into(),
                resource_group: "cloud".into(),
                max_input_bytes: 1024,
                larm_provider: None,
                capabilities: vec!["reason".into()],
            },
        ];
        let unreachable = selection_input_for(
            &policy,
            &ReachabilitySnapshot {
                harness: Reachability::Unreachable,
                ..ReachabilitySnapshot::default()
            },
        );
        assert_eq!(
            unreachable.unreachable_actor_ids,
            HashSet::from(["larm".into()])
        );
        assert!(unreachable.cloud_allowed);
        let unknown = selection_input_for(&policy, &ReachabilitySnapshot::default());
        assert!(unknown.unreachable_actor_ids.is_empty());
    }
}
