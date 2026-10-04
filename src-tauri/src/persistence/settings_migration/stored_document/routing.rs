use super::*;

pub(crate) fn migrate_routing_document(value: &mut Value) {
    let Some(document) = value.as_object_mut() else {
        return;
    };
    if let Some(conversation) = document
        .get_mut("conversationRespond")
        .and_then(Value::as_object_mut)
    {
        if !conversation.contains_key("source") {
            let harness_selected = conversation
                .get("primaryProviderId")
                .and_then(Value::as_str)
                == Some("dynamic-lan-primary");
            conversation.insert(
                "source".to_string(),
                Value::String(
                    if harness_selected {
                        "harness"
                    } else {
                        "provider"
                    }
                    .to_string(),
                ),
            );
            if harness_selected {
                conversation.insert("primaryProviderId".to_string(), Value::Null);
                conversation.insert("fallbackProviderIds".to_string(), json!([]));
            }
        }
    }
    document
        .entry("voiceTranscribe")
        .or_insert_with(|| json!({ "source": "harness", "providerId": null, "timeoutMs": 120000 }));
    document.entry("voiceSpeak").or_insert_with(
        || json!({ "source": "provider", "providerId": "system-tts", "timeoutMs": 30000 }),
    );
}
pub(crate) fn migrate_obsolete_direct_lan_route(providers: &Value, routing: &mut Value) {
    let Some(conversation) = routing
        .get_mut("conversationRespond")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if conversation.get("source").and_then(Value::as_str) != Some("provider") {
        return;
    }
    let Some(primary_id) = conversation
        .get("primaryProviderId")
        .and_then(Value::as_str)
    else {
        return;
    };
    let Some(items) = providers.get("providers").and_then(Value::as_array) else {
        return;
    };
    let Some(dynamic) = items.iter().find(|provider| {
        provider.get("id").and_then(Value::as_str) == Some(DYNAMIC_LAN_PROVIDER_ID)
            && provider.get("kind").and_then(Value::as_str) == Some("dynamic-lan")
            && provider.get("enabled").and_then(Value::as_bool) == Some(true)
    }) else {
        return;
    };
    let Some(dynamic_host) = dynamic.get("host").and_then(Value::as_str) else {
        return;
    };
    let harness_matches = providers
        .pointer("/harness/address")
        .and_then(Value::as_str)
        .and_then(|address| url::Url::parse(address).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some(dynamic_host)
                && url.port() == Some(crate::providers::dynamic_lan::CONTROL_PORT)
                && url.path() == "/"
        });
    let direct_matches = items
        .iter()
        .find(|provider| provider.get("id").and_then(Value::as_str) == Some(primary_id))
        .filter(|provider| {
            provider.get("kind").and_then(Value::as_str) == Some("openai-compatible")
                && provider.get("location").and_then(Value::as_str) == Some("local")
                && provider.get("enabled").and_then(Value::as_bool) == Some(true)
        })
        .and_then(|provider| provider.get("endpoint").and_then(Value::as_str))
        .and_then(|endpoint| url::Url::parse(endpoint).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some(dynamic_host)
                && url.port_or_known_default() == Some(8_080)
        });
    if harness_matches && direct_matches {
        conversation.insert("source".to_string(), Value::String("harness".to_string()));
        conversation.insert("primaryProviderId".to_string(), Value::Null);
        conversation.insert("fallbackProviderIds".to_string(), json!([]));
    }
}
