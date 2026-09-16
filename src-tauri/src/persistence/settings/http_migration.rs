use serde_json::{json, Value};
pub(super) fn migrate_http_bases(document: &mut Value) {
    let Some(providers) = document.get_mut("providers").and_then(Value::as_array_mut) else {
        return;
    };
    for provider in providers {
        if !matches!(
            provider["kind"].as_str(),
            Some("openai-compatible" | "cloud-asr" | "cloud-tts")
        ) {
            continue;
        }
        let Some(base) = provider["endpoint"].as_str() else {
            continue;
        };
        let Ok(mut url) = url::Url::parse(base) else {
            continue;
        };
        let mut path = url.path().trim_end_matches('/').to_string();
        for suffix in [
            "/chat/completions",
            "/audio/transcriptions",
            "/audio/speech",
            "/models",
        ] {
            if path.ends_with(suffix) {
                path.truncate(path.len() - suffix.len());
                break;
            }
        }
        if !path.is_empty()
            && !path.ends_with("/v1")
            && !path.ends_with("/openai")
            && !path.rsplit('/').next().is_some_and(|part| {
                part.starts_with('v') && part.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
            })
        {
            path.push_str("/v1");
            url.set_path(&path);
            provider["endpoint"] = json!(url.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_vendor_prefixes_survive_and_legacy_proxy_shorthand_is_preserved_once() {
        let mut value = json!({"harness":{"address":"http://local:9810"},"providers":[{"kind":"openai-compatible","endpoint":"http://local/proxy"},{"kind":"cloud-asr","endpoint":"https://provider/v1beta/openai/"},{"kind":"cloud-tts","endpoint":"https://provider/v2"}]});
        migrate_http_bases(&mut value);
        assert_eq!(value["providers"][0]["endpoint"], "http://local/proxy/v1");
        assert_eq!(
            value["providers"][1]["endpoint"],
            "https://provider/v1beta/openai/"
        );
        assert_eq!(value["providers"][2]["endpoint"], "https://provider/v2");
        assert!(value["harness"].get("ttsVoice").is_none());
        let once = value.clone();
        migrate_http_bases(&mut value);
        assert_eq!(value, once);
    }
}
