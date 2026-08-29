use serde_json::Value;

use super::contracts::RunFailureCode;

#[derive(Debug)]
pub(crate) struct CodexTurnOutcome {
    pub(crate) thread_id: String,
    pub(crate) content: String,
    pub(crate) last_progress_at: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CodexTurnFailure {
    pub(crate) thread_id: Option<String>,
    pub(crate) message: String,
    pub(crate) code: RunFailureCode,
    pub(crate) last_progress_at: Option<String>,
}

#[derive(Debug)]
pub(crate) enum CodexReaderMessage {
    Message(Value),
    Failed {
        code: RunFailureCode,
        message: &'static str,
    },
}

#[derive(Debug)]
pub(crate) struct TurnCompletion;

#[derive(Debug)]
pub(crate) struct TurnExecutionFailure {
    pub(crate) code: RunFailureCode,
    pub(crate) message: String,
    pub(crate) supervisor_version: Option<&'static str>,
    pub(crate) last_progress_at: Option<String>,
    pub(crate) finalized: bool,
}

impl TurnExecutionFailure {
    pub(crate) fn unsupervised(code: RunFailureCode, message: String) -> Self {
        Self {
            code,
            message,
            supervisor_version: None,
            last_progress_at: None,
            finalized: false,
        }
    }

    pub(crate) fn configuration(message: impl Into<String>) -> Self {
        Self::unsupervised(RunFailureCode::ConfigurationError, message.into())
    }
}

impl From<String> for TurnExecutionFailure {
    fn from(message: String) -> Self {
        Self::unsupervised(RunFailureCode::InternalError, message)
    }
}
