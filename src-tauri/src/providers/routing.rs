use crate::{ConversationRouteSettings, ModelProvidersSettings, SecurityRuntimeSettings};

mod service_harness;
pub(crate) use service_harness::resolve_harness_llm_provider;

pub(crate) fn effective_conversation_route_ids(
    providers: &ModelProvidersSettings,
    route: &ConversationRouteSettings,
    security: &SecurityRuntimeSettings,
) -> Vec<String> {
    if route.source == "provider" && route.primary_provider_id.is_none() {
        return vec![];
    }
    let primary = providers
        .providers
        .iter()
        .find(|provider| Some(provider.id()) == route.primary_provider_id.as_deref());
    let primary_is_local =
        route.source == "harness" || primary.is_some_and(|provider| provider.location() == "local");
    let primary_id = if route.source == "harness" {
        Some(crate::DYNAMIC_LAN_PROVIDER_ID.to_string())
    } else {
        route.primary_provider_id.clone()
    };
    primary_id
        .into_iter()
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
    use crate::test_support::{dynamic_lan_provider, provider};
    use crate::{ConversationRouteSettings, ModelProvidersSettings, SecurityRuntimeSettings};

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
                larm_profile: None,
                tts_voice: None,
                tts_style: None,
                tts_speed: None,
                tts_pitch_scale: None,
                tts_intonation_scale: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        let route = ConversationRouteSettings {
            attempt_timeout_ms: None,
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
                larm_profile: None,
                tts_voice: None,
                tts_style: None,
                tts_speed: None,
                tts_pitch_scale: None,
                tts_intonation_scale: None,
                address: "http://localhost:9810".to_string(),
            },
        };
        let route = ConversationRouteSettings {
            attempt_timeout_ms: None,
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
}
