use super::{authorized, failed, safe_remote_id, send, session_operation_url, SessionResponse};
use crate::providers::stream::{
    CleanupOutcome, ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind,
};
use crate::{
    ipc_contract::{ConversationMessage, RuntimeEvent},
    AgentSessionProviderSettings,
};
use futures_util::StreamExt;
use reqwest::{header, Client, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::time::Instant as TokioInstant;
use url::Url;
mod coding_bridge;
mod fresh_session;
mod transport;
use transport::{idempotency_key, is_event_stream, read_turn};
mod generation;
mod request;
use request::{render_turn_input, start_turn};
mod probe;
mod ui_bridge;
#[cfg(test)]
mod workflow_tests;
mod world_eval_tests;
pub(super) use probe::probe_event_stream;
include!("sse.d/01.rs");
include!("sse.d/02.rs");
