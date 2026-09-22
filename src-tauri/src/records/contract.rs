use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RecordKind {
    WebSearch,
    WebSearchResult,
    WebFetch,
    ToolResult,
    McpResult,
    ConversationMessageRef,
}

impl RecordKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WebSearch => "web_search",
            Self::WebSearchResult => "web_search_result",
            Self::WebFetch => "web_fetch",
            Self::ToolResult => "tool_result",
            Self::McpResult => "mcp_result",
            Self::ConversationMessageRef => "conversation_message_ref",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "web_search" | "webSearch" => Self::WebSearch,
            "web_search_result" | "webSearchResult" => Self::WebSearchResult,
            "web_fetch" | "webFetch" => Self::WebFetch,
            "tool_result" | "toolResult" => Self::ToolResult,
            "mcp_result" | "mcpResult" => Self::McpResult,
            "conversation_message_ref" | "conversationMessageRef" => Self::ConversationMessageRef,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Origin {
    UserStatement,
    ExternalObservation,
    RuntimeState,
    DerivedClaim,
}

impl Origin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UserStatement => "user_statement",
            Self::ExternalObservation => "external_observation",
            Self::RuntimeState => "runtime_state",
            Self::DerivedClaim => "derived_claim",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CaptureState {
    Streaming,
    Complete,
    Partial,
    Failed,
}

impl CaptureState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutlineItem {
    pub(crate) start_byte: u64,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outline {
    pub(crate) parser_version: &'static str,
    pub(crate) items: Vec<OutlineItem>,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordRef {
    pub(crate) id: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) outline: Option<Outline>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_21_kind_roundtrip() {
        let kind = RecordKind::WebSearchResult;
        assert_eq!(kind.as_str(), "web_search_result");
        assert_eq!(RecordKind::parse(kind.as_str()), Some(kind));
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"webSearchResult\"");
        assert_eq!(serde_json::from_str::<RecordKind>(&json).unwrap(), kind);
    }
}
