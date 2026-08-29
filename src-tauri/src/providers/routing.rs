use crate::{ConversationRouteSettings, ModelProvidersSettings, SecurityRuntimeSettings};

pub(crate) fn effective_conversation_route_ids(
    providers: &ModelProvidersSettings,
    route: &ConversationRouteSettings,
    security: &SecurityRuntimeSettings,
) -> Vec<String> {
    let primary = providers
        .providers
        .iter()
        .find(|provider| provider.id() == route.primary_provider_id);
    let primary_is_local = primary.is_some_and(|provider| provider.location() == "local");
    std::iter::once(route.primary_provider_id.clone())
        .chain(route.fallback_provider_ids.iter().cloned())
        .filter(|provider_id| {
            !(security.local_only_when_selected && primary_is_local)
                || providers
                    .providers
                    .iter()
                    .find(|provider| provider.id() == *provider_id)
                    .is_none_or(|provider| provider.location() == "local")
        })
        .collect()
}

pub(crate) fn apply_runtime_provider_gates(
    providers: &ModelProvidersSettings,
    route_ids: Vec<String>,
    larm_gate: &crate::providers::larm::LarmRuntimeGate,
) -> Vec<String> {
    route_ids
        .into_iter()
        .filter(|provider_id| {
            providers
                .providers
                .iter()
                .find(|provider| provider.id() == provider_id)
                .is_none_or(|provider| provider.kind() != "larm" || larm_gate.allows_traffic())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dynamic_lan_provider, larm_provider, provider};
    use crate::{ConversationRouteSettings, ModelProvidersSettings, SecurityRuntimeSettings};
    use std::sync::Arc;

    #[test]
    fn local_only_route_excludes_cloud_fallback() {
        let providers = ModelProvidersSettings {
            providers: vec![
                provider("local-primary", "local"),
                provider("cloud-fallback", "cloud"),
                provider("local-fallback", "local"),
            ],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        };
        let route = ConversationRouteSettings {
            primary_provider_id: "local-primary".to_string(),
            fallback_provider_ids: vec!["cloud-fallback".to_string(), "local-fallback".to_string()],
            timeout_ms: 30_000,
        };
        let security = SecurityRuntimeSettings {
            credential_storage: "environment".to_string(),
            local_only_when_selected: true,
            diagnostics_redaction: true,
        };

        assert_eq!(
            effective_conversation_route_ids(&providers, &route, &security),
            ["local-primary", "local-fallback"]
        );
    }

    #[test]
    fn dynamic_lan_route_keeps_local_fallbacks_at_runtime() {
        let providers = ModelProvidersSettings {
            providers: vec![
                dynamic_lan_provider("dynamic_lan-primary"),
                provider("local-fallback", "local"),
            ],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        };
        let route = ConversationRouteSettings {
            primary_provider_id: "dynamic_lan-primary".to_string(),
            fallback_provider_ids: vec!["local-fallback".to_string()],
            timeout_ms: 30_000,
        };
        let security = SecurityRuntimeSettings {
            credential_storage: "environment".to_string(),
            local_only_when_selected: true,
            diagnostics_redaction: true,
        };

        assert_eq!(
            effective_conversation_route_ids(&providers, &route, &security),
            ["dynamic_lan-primary", "local-fallback"]
        );
    }

    #[test]
    fn disabled_larm_gate_removes_only_larm_and_preserves_direct_rollback_order() {
        let providers = ModelProvidersSettings {
            providers: vec![
                larm_provider("larm-primary"),
                provider("direct-rollback", "local"),
            ],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        };
        let configured = vec!["larm-primary".to_string(), "direct-rollback".to_string()];
        assert_eq!(
            apply_runtime_provider_gates(
                &providers,
                configured.clone(),
                &crate::providers::larm::LarmRuntimeGate::Disabled,
            ),
            vec!["direct-rollback"]
        );
        let ready = crate::providers::larm::LarmRuntimeGate::Ready(Arc::new(
            crate::providers::larm::client::SharedLarmClient::build().expect("LARM client builds"),
        ));
        assert_eq!(
            apply_runtime_provider_gates(&providers, configured.clone(), &ready),
            configured
        );
    }
}
