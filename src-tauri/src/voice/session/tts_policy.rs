use super::TtsRoute;
use crate::{ModelProvidersSettings, SecurityRuntimeSettings, VoiceRouteSettings};
pub(super) fn wrap(
    primary: TtsRoute,
    providers: &ModelProvidersSettings,
    route: &VoiceRouteSettings,
    security: &SecurityRuntimeSettings,
) -> TtsRoute {
    let primary_local = route.source == "harness"
        || providers
            .providers
            .iter()
            .any(|p| Some(p.id()) == route.provider_id.as_deref() && p.location() == "local");

    let mut routes = vec![primary];
    for id in &route.fallback_provider_ids {
        if let Some(p) = providers.providers.iter().find(|p| {
            p.id() == id
                && p.enabled()
                && !(security.local_only_when_selected && primary_local && p.location() == "cloud")
        }) {
            match p {
                crate::ModelProviderSettings::CloudTts(p) => {
                    routes.push(TtsRoute::Cloud(p.clone()))
                }
                crate::ModelProviderSettings::SystemTts(p) => {
                    routes.push(TtsRoute::System(p.clone()))
                }
                _ => {}
            }
        }
    }
    if routes.len() == 1
        && route.attempt_timeout_ms.is_none()
        && matches!(routes[0], TtsRoute::System(_))
    {
        routes.remove(0)
    } else {
        let budget = route
            .attempt_timeout_ms
            .unwrap_or(route.timeout_ms / routes.len() as u64);
        TtsRoute::Fallback(routes, budget)
    }
}
