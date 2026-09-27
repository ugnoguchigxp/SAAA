use serde::{Deserialize, Serialize};

pub const BLUETOOTH_TRANSPORT: u32 = u32::from_be_bytes(*b"blue");
pub const AIRPLAY_TRANSPORT: u32 = u32::from_be_bytes(*b"airp");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuckingLevel {
    Default,
    Min,
    Mid,
    Max,
}

impl DuckingLevel {
    pub fn parse(value: &str) -> Self {
        match value {
            "default" => Self::Default,
            "mid" => Self::Mid,
            "max" => Self::Max,
            _ => Self::Min,
        }
    }

    pub fn as_vpio_level(self) -> u32 {
        match self {
            Self::Default => 0,
            Self::Min => 10,
            Self::Mid => 20,
            Self::Max => 30,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Min => "min",
            Self::Mid => "mid",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceProcessingConfig {
    pub aec_enabled: bool,
    pub ducking: DuckingLevel,
    pub vpio_on_bluetooth: bool,
    pub bypass: bool,
}

impl Default for VoiceProcessingConfig {
    fn default() -> Self {
        Self {
            aec_enabled: true,
            ducking: DuckingLevel::Min,
            vpio_on_bluetooth: false,
            bypass: false,
        }
    }
}

impl VoiceProcessingConfig {
    pub fn from_settings(aec_enabled: bool, ducking: &str, vpio_on_bluetooth: bool) -> Self {
        Self {
            aec_enabled,
            ducking: DuckingLevel::parse(ducking),
            vpio_on_bluetooth,
            bypass: !aec_enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioBackendStatus {
    pub available: bool,
    pub reason: Option<String>,
    pub capture_active: bool,
    pub playback_active: bool,
    pub aec_active: bool,
    pub ducking_level: String,
    pub agc_enabled: bool,
    pub output_transport: Option<String>,
    pub macos_major: u8,
}
