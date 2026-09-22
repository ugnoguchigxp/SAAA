use serde::{Deserialize, Serialize};
use ts_rs::{Config, TS};

pub(crate) const PREVIEW_SCHEME: &str = "saaa-artifact-preview";
pub(crate) const MAX_HTML_BYTES: usize = 1_048_576;
pub(crate) const MAX_ACTIVE_TOKENS: usize = 8;
pub(crate) const TOKEN_TTL_MS: i64 = 120_000;
pub(crate) const APP_SCOPE: &str = "app:local";
pub(crate) const HTML_MEDIA_TYPE: &str = "text/html";

pub(crate) const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'none'; font-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; worker-src 'none'; manifest-src 'none'; form-action 'none'; base-uri 'none'";

pub(crate) const PERMISSIONS_POLICY: &str = "camera=(), microphone=(), geolocation=(), notifications=(), clipboard-read=(), clipboard-write=()";

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PrepareArtifactPreviewInput {
    pub artifact_id: String,
    pub revision_id: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReleaseArtifactPreviewInput {
    pub preview_token: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MountArtifactPreviewInput {
    pub preview_token: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArtifactPreviewPolicy {
    pub network: String,
    pub navigation: String,
    pub popup: String,
    pub download: String,
    pub tauri_ipc: String,
}

impl ArtifactPreviewPolicy {
    pub(crate) fn isolated() -> Self {
        Self {
            network: "none".into(),
            navigation: "preview-only".into(),
            popup: "deny".into(),
            download: "deny".into(),
            tauri_ipc: "deny".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArtifactPreviewDescriptor {
    pub artifact_id: String,
    pub revision_id: String,
    pub title: String,
    pub media_type: String,
    pub digest: String,
    pub preview_token: String,
    pub expires_at: String,
    pub webview_label: String,
    pub policy: ArtifactPreviewPolicy,
}

pub fn typescript_bindings() -> String {
    let types = [
        format!(
            "export {}",
            PrepareArtifactPreviewInput::decl(&Config::default())
        ),
        format!(
            "export {}",
            ReleaseArtifactPreviewInput::decl(&Config::default())
        ),
        format!(
            "export {}",
            MountArtifactPreviewInput::decl(&Config::default())
        ),
        format!("export {}", ArtifactPreviewPolicy::decl(&Config::default())),
        format!(
            "export {}",
            ArtifactPreviewDescriptor::decl(&Config::default())
        ),
    ]
    .join("\n\n");
    format!("// Generated from artifact_preview/contracts.rs. Do not edit.\n{types}\n")
}
