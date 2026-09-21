//! Voice text projection. The backchannel model is a classifier only and never
//! generates or rewrites user-visible text.
use crate::RunCancellation;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseKind {
    Acknowledgement,
    Thinking,
    ToolProgress,
    Final,
}

impl ResponseKind {
    pub(crate) fn sequence(self) -> u8 {
        match self {
            Self::Acknowledgement => 0,
            Self::Thinking => 1,
            Self::ToolProgress => 2,
            Self::Final => 3,
        }
    }
}

pub(crate) async fn render(
    _conversation_id: &str,
    _kind: ResponseKind,
    canonical_text: &str,
    _language: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<String, &'static str> {
    if cancellation.is_cancelled() {
        return Err("Response rendering cancelled");
    }
    let canonical_text = canonical_text.trim();
    if canonical_text.is_empty() {
        return Err("Response rendering input is invalid");
    }
    Ok(canonical_text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn final_text_is_not_rewritten() {
        let cancellation = Arc::new(RunCancellation::default());
        let canonical = "Qwen が生成した最終回答。数値 230400 を保持する。";
        assert_eq!(
            render(
                "conversation",
                ResponseKind::Final,
                canonical,
                "ja",
                cancellation
            )
            .await,
            Ok(canonical.into())
        );
    }
}
