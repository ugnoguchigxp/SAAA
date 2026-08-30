use crate::{ConversationRouteSettings, ModelProvidersSettings, SecurityRuntimeSettings};

mod runtime_gates;
mod service_harness;
pub(crate) use runtime_gates::apply_runtime_provider_gates;
pub(crate) use service_harness::resolve_harness_llm_provider;

pub(crate) fn effective_conversation_route_ids(
    providers: &ModelProvidersSettings,
    route: &ConversationRouteSettings,
    security: &SecurityRuntimeSettings,
) -> Vec<String> {
    if route.source == "harness" {
        return providers
            .providers
            .iter()
            .find(|provider| {
                provider.id() == crate::DYNAMIC_LAN_PROVIDER_ID
                    || matches!(provider, crate::ModelProviderSettings::DynamicLan(_))
            })
            .map(|provider| vec![provider.id().to_string()])
            .unwrap_or_default();
    }
    let primary = providers
        .providers
        .iter()
        .find(|provider| Some(provider.id()) == route.primary_provider_id.as_deref());
    let primary_is_local = primary.is_some_and(|provider| provider.location() == "local");
    route
        .primary_provider_id
        .iter()
        .cloned()
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
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        let route = ConversationRouteSettings {
            source: "provider".to_string(),
            primary_provider_id: Some("local-primary".to_string()),
            fallback_provider_ids: vec!["cloud-fallback".to_string(), "local-fallback".to_string()],
            timeout_ms: 30_000,
        };
        let security = SecurityRuntimeSettings {
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
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        let route = ConversationRouteSettings {
            source: "provider".to_string(),
            primary_provider_id: Some("dynamic_lan-primary".to_string()),
            fallback_provider_ids: vec!["local-fallback".to_string()],
            timeout_ms: 30_000,
        };
        let security = SecurityRuntimeSettings {
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
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
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
