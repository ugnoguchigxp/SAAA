use crate::ModelProvidersSettings;

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
