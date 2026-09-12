use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationMessage {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    #[ts(type = "\"user\" | \"assistant\" | \"system\" | \"transcript\"")]
    pub(crate) role: String,
    pub(crate) content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) parts: Option<Vec<crate::generative_ui::contracts::ContentPart>>,
    pub(crate) created_at: String,
}
