use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ContentPart {
    Text {
        text: String,
    },
    Ui {
        instance_id: String,
        view_id: String,
        revision: u32,
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UiNode {
    #[serde(default)]
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "full_span")]
    pub span: u8,
    #[serde(default)]
    pub children: Vec<UiNode>,
}

fn full_span() -> u8 {
    12
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiInstance {
    pub id: String,
    pub view_id: String,
    pub revision: u32,
    pub summary: String,
    pub definition: String,
    pub library_version: u32,
    pub mode: String,
    pub node: UiNode,
    #[ts(type = "Record<string, string | number | boolean>")]
    pub state: serde_json::Value,
    #[ts(type = "Record<string, UiData>")]
    pub snapshots: serde_json::Value,
    pub state_version: u32,
    pub name: Option<String>,
    pub published_revision: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiData {
    pub captured_at: String,
    #[ts(type = "Array<Record<string, string | number | null>>")]
    pub rows: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiViewRevision {
    pub revision: u32,
    pub summary: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PresentInput {
    #[serde(deserialize_with = "definition_input")]
    pub definition: String,
    pub summary: String,
    #[serde(default = "live")]
    pub mode: String,
    pub base_instance_id: Option<String>,
}
fn live() -> String {
    "live".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveInput {
    pub instance_id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn definition_input<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(text) => Ok(text),
        serde_json::Value::Object(_) => Ok(value.to_string()),
        _ => Err(serde::de::Error::custom("Expected a semantic node object")),
    }
}
