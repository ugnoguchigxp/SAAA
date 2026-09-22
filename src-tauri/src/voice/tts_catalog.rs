use serde::{Deserialize, Serialize};

const MAX_VOICES: usize = 512;
const MAX_STYLES: usize = 64;
const MAX_ID: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TtsVoiceCatalog {
    pub(crate) default_voice: Option<String>,
    pub(crate) voices: Vec<TtsVoice>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TtsVoice {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) voice_presentation: Option<String>,
    pub(crate) default_style: Option<String>,
    pub(crate) styles: Vec<TtsStyle>,
    pub(crate) credit: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TtsStyle {
    pub(crate) id: String,
    pub(crate) display_name: String,
}

#[derive(Debug)]
pub(crate) enum CatalogError {
    Protocol,
    #[allow(dead_code)]
    UnsupportedModel,
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol => formatter.write_str("catalog-protocol"),
            Self::UnsupportedModel => formatter.write_str("catalog-unsupported-model"),
        }
    }
}

#[derive(Deserialize)]
struct WireCatalog {
    #[serde(default)]
    default_voice: Option<String>,
    #[serde(default)]
    voices: Vec<WireVoice>,
}

#[derive(Deserialize)]
struct WireVoice {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    voice_presentation: Option<String>,
    #[serde(default)]
    default_style: Option<String>,
    #[serde(default)]
    styles: Vec<WireStyle>,
    #[serde(default)]
    capabilities: Option<WireCapabilities>,
    #[serde(default)]
    credit: Option<String>,
}

#[derive(Deserialize)]
struct WireStyle {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct WireCapabilities {
    #[serde(default)]
    speed: Option<WireRange>,
    #[serde(default)]
    pitch_scale: Option<WireRange>,
    #[serde(default)]
    intonation_scale: Option<WireRange>,
}

#[derive(Deserialize)]
struct WireRange {
    min: f64,
    max: f64,
}

fn clean_text(value: &str, max: usize) -> Result<String, CatalogError> {
    if value.is_empty()
        || value.chars().count() > max
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CatalogError::Protocol);
    }
    Ok(value.to_string())
}

fn range_ok(range: &WireRange, min: f64, max: f64) -> bool {
    range.min.is_finite()
        && range.max.is_finite()
        && range.min <= range.max
        && range.min >= min
        && range.max <= max
}

pub(crate) fn normalize_catalog(bytes: &[u8]) -> Result<TtsVoiceCatalog, CatalogError> {
    if bytes.len() > 1024 * 1024 {
        return Err(CatalogError::Protocol);
    }
    let wire: WireCatalog = serde_json::from_slice(bytes).map_err(|_| CatalogError::Protocol)?;
    if wire.voices.len() > MAX_VOICES {
        return Err(CatalogError::Protocol);
    }
    let mut voices = Vec::with_capacity(wire.voices.len());
    let mut seen = std::collections::HashSet::new();
    for voice in wire.voices {
        let id = clean_text(&voice.id, MAX_ID)?;
        if !seen.insert(id.clone()) {
            return Err(CatalogError::Protocol);
        }
        if voice.styles.len() > MAX_STYLES {
            return Err(CatalogError::Protocol);
        }
        let mut styles = Vec::new();
        let mut style_ids = std::collections::HashSet::new();
        for style in voice.styles {
            let style_id = clean_text(&style.id, MAX_ID)?;
            if !style_ids.insert(style_id.clone()) {
                return Err(CatalogError::Protocol);
            }
            let display_name = match style.display_name {
                Some(name) => clean_text(&name, MAX_ID)?,
                None => style_id.clone(),
            };
            styles.push(TtsStyle {
                id: style_id,
                display_name,
            });
        }
        if let Some(default_style) = &voice.default_style {
            let default_style = clean_text(default_style, MAX_ID)?;
            if !style_ids.contains(&default_style) {
                return Err(CatalogError::Protocol);
            }
        }
        if let Some(capabilities) = &voice.capabilities {
            let ok = capabilities
                .speed
                .as_ref()
                .is_none_or(|range| range_ok(range, 0.5, 2.0))
                && capabilities
                    .pitch_scale
                    .as_ref()
                    .is_none_or(|range| range_ok(range, -0.15, 0.15))
                && capabilities
                    .intonation_scale
                    .as_ref()
                    .is_none_or(|range| range_ok(range, 0.0, 2.0));
            if !ok {
                return Err(CatalogError::Protocol);
            }
        }
        let presentation = match voice.voice_presentation {
            Some(value) => Some(clean_text(&value, 80)?),
            None => None,
        };
        let credit = match voice.credit {
            Some(value) => Some(clean_text(&value, 512)?),
            None => None,
        };
        voices.push(TtsVoice {
            display_name: voice
                .display_name
                .as_deref()
                .map(|name| clean_text(name, MAX_ID))
                .transpose()?
                .unwrap_or_else(|| id.clone()),
            id,
            voice_presentation: presentation,
            default_style: voice.default_style,
            styles,
            credit,
        });
    }
    if let Some(default_voice) = &wire.default_voice {
        let default_voice = clean_text(default_voice, MAX_ID)?;
        if !seen.contains(&default_voice) {
            return Err(CatalogError::Protocol);
        }
    }
    Ok(TtsVoiceCatalog {
        default_voice: wire.default_voice,
        voices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ve_03_rich_and_simple_catalogs_normalize() {
        let rich = r#"{
          "default_voice":"Kasukabe_Tsumugi",
          "voices":[{
            "id":"Kasukabe_Tsumugi",
            "display_name":"春日部つむぎ",
            "voice_presentation":"feminine",
            "default_style":"normal",
            "styles":[{"id":"normal","display_name":"ノーマル"}],
            "capabilities":{"speed":{"min":0.5,"max":2.0},"pitch_scale":{"min":-0.15,"max":0.15},"intonation_scale":{"min":0.0,"max":2.0}},
            "credit":"VOICEVOX:春日部つむぎ"
          }]
        }"#;
        let catalog = normalize_catalog(rich.as_bytes()).unwrap();
        assert_eq!(catalog.default_voice.as_deref(), Some("Kasukabe_Tsumugi"));
        assert_eq!(catalog.voices[0].display_name, "春日部つむぎ");
        let simple = br#"{"voices":[{"id":"A"}]}"#;
        let catalog = normalize_catalog(simple).unwrap();
        assert_eq!(catalog.voices[0].display_name, "A");
        assert!(catalog.voices[0].styles.is_empty());
        assert!(catalog.voices[0].credit.is_none());
    }

    #[test]
    fn ve_03_catalog_rejects_duplicates_invalid_defaults_and_bounds() {
        assert!(normalize_catalog(br#"{"voices":[{"id":"A"},{"id":"A"}]}"#).is_err());
        assert!(
            normalize_catalog(br#"{"default_voice":"missing","voices":[{"id":"A"}]}"#).is_err()
        );
        assert!(normalize_catalog(
            br#"{"voices":[{"id":"A","default_style":"missing","styles":[]}]}"#
        )
        .is_err());
        assert!(normalize_catalog(
            br#"{"voices":[{"id":"A","capabilities":{"speed":{"min":0.1,"max":3.0}}}]}"#
        )
        .is_err());
    }
}
