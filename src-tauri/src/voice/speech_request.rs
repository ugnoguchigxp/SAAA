use serde::Serialize;

use crate::CloudTtsProviderSettings;

#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    response_format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pitch_scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    intonation_scale: Option<f64>,
}

impl<'a> SpeechRequest<'a> {
    pub(crate) fn from_provider(provider: &'a CloudTtsProviderSettings, input: &'a str) -> Self {
        let voicevox = provider.model == "voicevox-core";
        Self {
            model: &provider.model,
            input,
            voice: &provider.voice,
            response_format: &provider.response_format,
            style: voicevox.then_some(provider.style.as_deref()).flatten(),
            speed: voicevox.then_some(provider.speed).flatten(),
            pitch_scale: voicevox.then_some(provider.pitch_scale).flatten(),
            intonation_scale: voicevox.then_some(provider.intonation_scale).flatten(),
        }
    }
}

pub(crate) fn speech_request_value(
    provider: &CloudTtsProviderSettings,
    input: &str,
) -> serde_json::Value {
    serde_json::to_value(SpeechRequest::from_provider(provider, input))
        .expect("speech request is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(model: &str) -> CloudTtsProviderSettings {
        CloudTtsProviderSettings {
            id: "tts".into(),
            enabled: true,
            label: "TTS".into(),
            location: "local".into(),
            endpoint: "http://127.0.0.1/v1".into(),
            model: model.into(),
            voice: "Kasukabe_Tsumugi".into(),
            response_format: "wav".into(),
            authentication: "none".into(),
            style: Some("normal".into()),
            speed: Some(1.1),
            pitch_scale: Some(0.01),
            intonation_scale: Some(1.2),
        }
    }

    #[test]
    fn ve_02_cloud_voicevox_request_maps_all_fields() {
        let body = speech_request_value(&sample("voicevox-core"), "こんにちは");
        assert_eq!(body["style"], "normal");
        assert_eq!(body["speed"], 1.1);
        assert_eq!(body["pitch_scale"], 0.01);
        assert_eq!(body["intonation_scale"], 1.2);
        assert_eq!(body["voice"], "Kasukabe_Tsumugi");
    }

    #[test]
    fn ve_02_unset_and_non_voicevox_fields_are_omitted() {
        let mut provider = sample("voicevox-core");
        provider.style = None;
        provider.speed = None;
        provider.pitch_scale = None;
        provider.intonation_scale = None;
        let body = speech_request_value(&provider, "hello");
        assert!(body.get("style").is_none());
        assert!(body.get("speed").is_none());
        assert!(body.get("pitch_scale").is_none());
        assert!(body.get("intonation_scale").is_none());
        let body = speech_request_value(&sample("other-model"), "hello");
        assert!(body.get("style").is_none());
        assert!(body.get("speed").is_none());
        assert_eq!(body["model"], "other-model");
        assert_eq!(body["voice"], "Kasukabe_Tsumugi");
    }

    #[test]
    fn ve_02_existing_voice_only_request_is_unchanged() {
        let mut provider = sample("voicevox-core");
        provider.style = None;
        provider.speed = None;
        provider.pitch_scale = None;
        provider.intonation_scale = None;
        let body = speech_request_value(&provider, "Connectivity check");
        assert_eq!(
            body,
            serde_json::json!({
                "model": "voicevox-core",
                "input": "Connectivity check",
                "voice": "Kasukabe_Tsumugi",
                "response_format": "wav"
            })
        );
    }

    #[test]
    fn ve_02_larm_settings_map_all_fields() {
        let harness = crate::HarnessSettings {
            larm_profile: None,
            tts_voice: Some("Kasukabe_Tsumugi".into()),
            tts_style: Some("normal".into()),
            tts_speed: Some(0.8),
            tts_pitch_scale: Some(-0.05),
            tts_intonation_scale: Some(1.2),
            address: "http://127.0.0.1:9810".into(),
        };
        let mut provider = sample("voicevox-core");
        apply_harness_prosody(&mut provider, &harness);
        let body = speech_request_value(&provider, "音声設定の確認です。");
        assert_eq!(body["style"], "normal");
        assert_eq!(body["speed"], 0.8);
        assert_eq!(body["pitch_scale"], -0.05);
        assert_eq!(body["intonation_scale"], 1.2);
    }
}

pub(crate) fn apply_harness_prosody(
    provider: &mut CloudTtsProviderSettings,
    harness: &crate::HarnessSettings,
) {
    provider.style = harness.tts_style.clone();
    provider.speed = harness.tts_speed;
    provider.pitch_scale = harness.tts_pitch_scale;
    provider.intonation_scale = harness.tts_intonation_scale;
}
