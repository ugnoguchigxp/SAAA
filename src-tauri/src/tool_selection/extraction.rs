//! Scenario/correction extraction boundary. The production implementation calls the already
//! configured conversation provider with one tool-less structured request; D0–D3 tests use the
//! deterministic fixture extractor, which the guide explicitly allows for hand-checked fixtures.

use async_trait::async_trait;

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
    /// Full host-validated reference set used to validate the model output.
    pub allowed_decisions: std::collections::HashSet<String>,
    pub allowed_tools: std::collections::HashSet<String>,
    /// Small ordered list of tool names actually shown to the model.
    pub prompt_tools: Vec<String>,
}

#[async_trait]
pub trait CorrectionExtractor: Send + Sync {
    async fn extract(
        &self,
        request: ExtractionRequest,
    ) -> Result<ParsedExtraction, ExtractionFailure>;
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
    async fn extract(
        &self,
        request: ExtractionRequest,
    ) -> Result<ParsedExtraction, ExtractionFailure> {
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
    async fn extract(
        &self,
        _request: ExtractionRequest,
    ) -> Result<ParsedExtraction, ExtractionFailure> {
        Err(ExtractionFailure::UnexpectedShape)
    }
}
