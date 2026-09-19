//! Scenario/correction extraction boundary. The production implementation calls the already
//! configured conversation provider with one tool-less structured request; D0–D3 tests use the
//! deterministic fixture extractor, which the guide explicitly allows for hand-checked fixtures.

use async_trait::async_trait;
use std::sync::Arc;

use super::feedback::{parse_extraction, ExtractionFailure, ParsedExtraction};

#[derive(Clone, Debug)]
pub struct RecentDecision {
    pub decision_id: String,
    pub scenario_summary: String,
    pub tool_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ExtractionRequest {
    pub user_message: String,
    pub recent_decisions: Vec<RecentDecision>,
    pub allowed_decisions: std::collections::HashSet<String>,
    pub allowed_tools: std::collections::HashSet<String>,
}

#[async_trait]
pub trait CorrectionExtractor: Send + Sync {
    async fn extract(&self, request: ExtractionRequest) -> Result<ParsedExtraction, ExtractionFailure>;
}

/// Deterministic extractor used by the golden fixtures. It returns a fixed JSON body, after
/// running the same strict host validation as the production path.
pub struct FixtureExtractor {
    body: String,
}

impl FixtureExtractor {
    pub fn new(body: &str) -> Self {
        Self {
            body: body.to_string(),
        }
    }
}

#[async_trait]
impl CorrectionExtractor for FixtureExtractor {
    async fn extract(&self, request: ExtractionRequest) -> Result<ParsedExtraction, ExtractionFailure> {
        parse_extraction(
            &self.body,
            &request.user_message,
            &request.allowed_decisions,
            &request.allowed_tools,
        )
    }
}

/// Used when no conversation provider is configured. Extraction fails closed: the scenario
/// degrades and no feedback is invented.
pub struct UnconfiguredExtractor;

#[async_trait]
impl CorrectionExtractor for UnconfiguredExtractor {
    async fn extract(&self, _request: ExtractionRequest) -> Result<ParsedExtraction, ExtractionFailure> {
        Err(ExtractionFailure::UnexpectedShape)
    }
}

/// Adapter over any async closure, so the conversation provider can be injected without this
/// module depending on the provider stack.
pub struct ClosureExtractor<F> {
    function: Arc<F>,
}

impl<F> ClosureExtractor<F> {
    pub fn new(function: F) -> Self {
        Self {
            function: Arc::new(function),
        }
    }
}

#[async_trait]
impl<F> CorrectionExtractor for ClosureExtractor<F>
where
    F: Fn(String) -> futures_util::future::BoxFuture<'static, Result<String, ExtractionFailure>>
        + Send
        + Sync,
{
    async fn extract(&self, request: ExtractionRequest) -> Result<ParsedExtraction, ExtractionFailure> {
        let body = (self.function)(request.user_message.clone()).await?;
        parse_extraction(
            &body,
            &request.user_message,
            &request.allowed_decisions,
            &request.allowed_tools,
        )
    }
}
