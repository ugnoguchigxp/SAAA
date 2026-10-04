use super::*;
pub(super) fn selected_provider(
    state: &AppState,
) -> Result<Option<crate::CloudAsrProviderSettings>, String> {
    state.sqlite_readers.read(|connection| {
        let loaded = persistence::service_registry_store::load_registry(connection)?;
        let route = crate::providers::service_registry::resolve_route(
            &loaded.snapshot,
            crate::providers::service_registry::Purpose::VoiceTranscribe,
        )
        .map_err(|_| "音声入力の用途設定が無効です")?;
        if route.resource_id.starts_with("res:svc-") || route.connection_id == "conn:harness" {
            return Ok(None);
        }
        let provider_id = route
            .resource_id
            .strip_prefix("res:")
            .ok_or("音声モデルIDが不正です")?;
        let providers = persistence::load_model_providers(connection)?;
        let selected = providers
            .providers
            .into_iter()
            .find(|provider| provider_id == provider.id() && provider.enabled());
        match selected {
            Some(ModelProviderSettings::CloudAsr(provider)) => Ok(Some(provider)),
            _ => Err("設定済みのASR Providerが見つかりません。".into()),
        }
    })
}
