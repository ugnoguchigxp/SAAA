use super::types::*;
use std::collections::HashSet;

/// Cross-reference validation for a whole snapshot. Disabled resources may stay
/// bound (the user disabled them); resolution rejects them at run time.
pub fn validate_snapshot(snapshot: &RegistrySnapshot) -> Result<(), String> {
    let mut connection_ids = HashSet::new();
    for connection in &snapshot.connections {
        if !matches!(connection.location.as_str(), "local" | "cloud") {
            return Err("Service location must be local or cloud".into());
        }
        match connection.authentication.as_str() {
            "none" if connection.credential_ref.is_none() => {}
            "api-key" if connection.credential_ref.is_some() => {}
            _ => return Err("Service authentication and credential reference disagree".into()),
        }
        if (connection.enabled || connection.connection_id.starts_with("conn:svc-"))
            && matches!(
                connection.adapter_kind,
                AdapterKind::ChatCompletions
                    | AdapterKind::AnthropicMessages
                    | AdapterKind::ReplicateMedia
                    | AdapterKind::HttpAsr
                    | AdapterKind::HttpTts
            )
        {
            saaa_larm_session::http_api::operation_url(&connection.endpoint, "models")?;
        }
        if connection.connection_id.starts_with("conn:svc-") {
            if let Some(reference) = &connection.credential_ref {
                if reference.service != crate::SERVICE_CONNECTION_CREDENTIAL_SERVICE
                    || reference.account != connection.connection_id
                {
                    return Err(
                        "New connections must use their own service credential reference".into(),
                    );
                }
            }
        }
        if connection.connection_id.len() > 128
            || connection.label.len() > 256
            || connection.endpoint.len() > 4096
        {
            return Err("Service settings exceed the supported size".into());
        }
        if connection.connection_id.trim().is_empty()
            || !connection_ids.insert(connection.connection_id.as_str())
        {
            return Err(format!(
                "Invalid or duplicate connection id: {}",
                connection.connection_id
            ));
        }
    }
    let mut resource_ids = HashSet::new();
    for resource in &snapshot.resources {
        if resource.resource_id.len() > 128
            || resource.model.len() > 256
            || resource.detail.as_ref().is_some_and(|v| v.len() > 8192)
        {
            return Err("Resource settings exceed the supported size".into());
        }
        if resource.resource_id.trim().is_empty()
            || !resource_ids.insert(resource.resource_id.as_str())
        {
            return Err(format!(
                "Invalid or duplicate resource id: {}",
                resource.resource_id
            ));
        }
        if snapshot
            .connection(&resource.connection_id)
            .is_some_and(|c| c.adapter_kind == AdapterKind::AnthropicMessages)
            && resource.request_options.is_some()
        {
            return Err("Messages形式にはChat Completions専用設定を適用できません".into());
        }
        if snapshot
            .connection(&resource.connection_id)
            .is_some_and(|c| c.adapter_kind == AdapterKind::ReplicateMedia)
        {
            crate::model_parts(&resource.model)?;
            if !matches!(
                resource.capability,
                Capability::ImageGeneration | Capability::MusicGeneration
            ) || resource.request_options.is_some()
            {
                return Err("Replicateの生成用途が不正です".into());
            }
            if let Some(detail) = resource.detail.as_deref().filter(|v| !v.trim().is_empty()) {
                if !serde_json::from_str::<serde_json::Value>(detail)
                    .map_err(|_| "生成パラメーターのJSONが不正です")?
                    .is_object()
                {
                    return Err("生成パラメーターはJSON objectで入力してください".into());
                }
            }
        }
        if let Some(options) = &resource.request_options {
            serde_json::from_value::<saaa_larm_session::http_api::LlmOptions>(options.clone())
                .map_err(|e| format!("Invalid model options for {}: {e}", resource.resource_id))?;
        }
        if !connection_ids.contains(resource.connection_id.as_str()) {
            return Err(format!(
                "Resource references an unknown connection: {}",
                resource.resource_id
            ));
        }
    }
    let mut purposes = HashSet::new();
    for binding in &snapshot.bindings {
        if !purposes.insert(binding.purpose) {
            return Err(format!("Duplicate binding: {}", binding.purpose.id()));
        }
        let mut seen = HashSet::new();
        let ids = binding
            .primary_resource_id
            .iter()
            .chain(binding.fallback_resource_ids.iter());
        for id in ids {
            let resource = snapshot.resource(id).ok_or_else(|| {
                format!(
                    "Binding {} references an unknown resource: {id}",
                    binding.purpose.id()
                )
            })?;
            if resource.capability != binding.purpose.required_capability() {
                return Err(format!(
                    "Resource {id} does not support {}",
                    binding.purpose.id()
                ));
            }
            if binding.review == BindingReview::Ready {
                if let Some(reason) =
                    crate::compatibility::unsupported_reason(snapshot, binding.purpose, id)
                {
                    return Err(format!("{}: {reason}", binding.purpose.id()));
                }
            }
            if !seen.insert(id.as_str()) {
                return Err(format!(
                    "Duplicate resource in {} route: {id}",
                    binding.purpose.id()
                ));
            }
        }
        let larm = |id: &str| {
            snapshot
                .resource(id)
                .and_then(|r| snapshot.connection(&r.connection_id))
                .is_some_and(|c| c.adapter_kind == AdapterKind::Larm)
        };
        if binding.review == BindingReview::Ready
            && binding.fallback_resource_ids.iter().any(|id| larm(id))
        {
            return Err(
                "LARMは代替先にできません。LARMが使えない時の代替先を指定してください".into(),
            );
        }
        if binding.primary_resource_id.is_none() && !binding.fallback_resource_ids.is_empty() {
            return Err("A fallback requires a primary resource".to_string());
        }
        if binding.timeout_ms > 3_600_000
            || binding
                .attempt_timeout_ms
                .is_some_and(|ms| ms > binding.timeout_ms)
        {
            return Err(
                "Attempt timeout must not exceed the total timeout (maximum one hour)".into(),
            );
        }
        if binding.attempt_timeout_ms == Some(0) {
            return Err("Attempt timeout must be positive".into());
        }
        if binding.timeout_ms == 0 {
            return Err(format!("Binding {} needs a timeout", binding.purpose.id()));
        }
    }
    Ok(())
}
