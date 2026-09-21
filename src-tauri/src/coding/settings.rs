use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingSettings {
    pub enabled: bool,
    pub executable: String,
    pub version: String,
    pub provider: String,
    pub model: String,
    pub profile: String,
    #[serde(default)]
    pub sdk_extension_path: Option<String>,
}
impl Default for CodingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: String::new(),
            version: "0.86.1".into(),
            provider: "openai-codex".into(),
            model: "gpt-5.6-luna".into(),
            profile: "trusted-local-v1".into(),
            sdk_extension_path: None,
        }
    }
}
pub fn valid_profile(settings: &CodingSettings) -> bool {
    match settings.profile.as_str() {
        "trusted-local-v1" => settings.sdk_extension_path.is_none(),
        "delegated-read-test-macos-v1" => {
            settings.sdk_extension_path.is_none()
                && cfg!(target_os = "macos")
                && std::path::Path::new("/usr/bin/sandbox-exec").is_file()
        }
        "delegated-codex-sdk-macos-v1" => {
            settings.provider == "saaa-codex-sdk"
                && settings.model == "gpt-5.6-luna"
                && cfg!(target_os = "macos")
                && std::path::Path::new("/usr/bin/sandbox-exec").is_file()
                && settings.sdk_extension_path.as_ref().is_some_and(|p| {
                    p.len() <= 4096
                        && std::path::Path::new(p).is_absolute()
                        && std::path::Path::new(p).is_file()
                })
        }
        "codex-sdk-v1" => {
            settings.provider == "saaa-codex-sdk"
                && settings.model == "gpt-5.6-luna"
                && settings.sdk_extension_path.as_ref().is_some_and(|p| {
                    p.len() <= 4096
                        && std::path::Path::new(p).is_absolute()
                        && std::path::Path::new(p).is_file()
                })
        }
        _ => false,
    }
}
