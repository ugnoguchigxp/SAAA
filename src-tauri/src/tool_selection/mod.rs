//! Tool-selection ledger: a SQLite-backed catalog of L-Lang capabilities and external MCP tools,
//! hybrid retrieval with a local ML worker, conditional user-correction memory, and the three
//! conversation entry points. D4 adds external MCP Streamable HTTP sources; D5 (publishing SAAA
//! itself as an MCP server) and D6 (ranking learning) remain out of scope.

pub mod backends;
pub mod catalog;
pub mod contracts;
pub mod extraction;
pub mod feedback;
pub mod gateway;
pub mod gateway_schemas;
pub mod inference;
pub mod mcp;
pub mod provider_extraction;
pub mod ranking;
pub mod references;
pub mod repository;
pub mod resolve;
pub mod retrieval;
pub mod rules;
pub mod schema;
pub mod service;
pub mod worker;

#[cfg(test)]
mod tests;

pub use contracts::{
    InputKind, ObjectType, Operation, Phase, RequestContext, Scenario, SelectionMode,
    ToolSelectionConfig, ToolSelectionError, ToolSelectionErrorCode, ToolSelectionResult,
};
pub use service::ToolSelectionService;

/// Builds the live service from the parsed configuration. Discovery loads only local model files;
/// a missing manifest degrades inference but never falls back to another model. External MCP
/// sources are wired whenever an `mcpSourcesPath` is configured, independent of discovery mode.
pub(crate) fn build_service(
    writer: std::sync::Arc<crate::persistence::SqliteWriter>,
    config: &ToolSelectionConfig,
    capabilities: Option<std::sync::Arc<crate::generated_capabilities::service::CapabilityService>>,
) -> ToolSelectionService {
    use std::sync::Arc;

    // A crash can leave invocations in `running`; settle them before serving again.
    let _ = service::reconcile_interrupted_invocations(&writer);
    let extractor: Arc<dyn extraction::CorrectionExtractor> = if config.discovery_enabled() {
        Arc::new(provider_extraction::ConversationProviderExtractor::new(
            writer.clone(),
        ))
    } else {
        Arc::new(extraction::UnconfiguredExtractor)
    };
    let (embedding, reranker, threshold): (
        Arc<dyn inference::EmbeddingProvider>,
        Arc<dyn inference::RerankProvider>,
        f64,
    ) = if config.discovery_enabled() {
        let manifest_path = config
            .model_manifest_path
            .clone()
            .expect("discovery requires a manifest path");
        let manifest = inference::load_manifest(&manifest_path);
        match (manifest, config.python_path.clone()) {
            (Ok(manifest), Some(python_path)) => {
                let script = worker_script_path();
                let worker = Arc::new(worker::MlWorker::new(
                    python_path,
                    script,
                    manifest_path,
                    manifest.clone(),
                ));
                (
                    worker.clone() as Arc<dyn inference::EmbeddingProvider>,
                    worker as Arc<dyn inference::RerankProvider>,
                    manifest.no_match_threshold,
                )
            }
            _ => (
                Arc::new(inference::UnavailableEmbedding),
                Arc::new(inference::UnavailableReranker),
                f64::NEG_INFINITY,
            ),
        }
    } else {
        (
            Arc::new(inference::UnavailableEmbedding),
            Arc::new(inference::UnavailableReranker),
            f64::NEG_INFINITY,
        )
    };
    let (backend, manager) =
        mcp::wiring::assemble(writer.clone(), config, capabilities, embedding.clone());
    let mut service =
        ToolSelectionService::new(writer, embedding, reranker, extractor, backend, threshold);
    // External MCP sources are usable through the same three entry points even when the local
    // discovery worker is not configured, so the definitions are exposed when either is present.
    service.set_discovery_configured(config.discovery_enabled() || manager.is_some());
    mcp::wiring::attach(&mut service, manager);
    service
}

/// Opens (or creates) a tool-selection database at `database_path` and builds the live service.
/// Used by the evaluation CLI and future management tooling.
pub fn open_service(
    database_path: &std::path::Path,
    config: &ToolSelectionConfig,
) -> ToolSelectionResult<ToolSelectionService> {
    let writer = crate::persistence::SqliteWriter::open(database_path)
        .map_err(|_| ToolSelectionError::storage())?;
    Ok(build_service(std::sync::Arc::new(writer), config, None))
}

/// Deterministic mock lane: hash embeddings and hash reranking. The evaluation CLI records
/// `lane=mock` so no one mistakes it for the real-model result.
pub fn open_mock_service(
    database_path: &std::path::Path,
) -> ToolSelectionResult<ToolSelectionService> {
    let writer = crate::persistence::SqliteWriter::open(database_path)
        .map_err(|_| ToolSelectionError::storage())?;
    let _ = service::reconcile_interrupted_invocations(&writer);
    let mut service = ToolSelectionService::new(
        std::sync::Arc::new(writer),
        std::sync::Arc::new(inference::HashEmbedding::new(384)),
        std::sync::Arc::new(inference::HashReranker::new()),
        std::sync::Arc::new(extraction::UnconfiguredExtractor),
        std::sync::Arc::new(backends::FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_discovery_configured(true);
    Ok(service)
}

fn worker_script_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("SAAA_TOOL_SELECTION_WORKER") {
        return std::path::PathBuf::from(path);
    }
    let base = option_env!("CARGO_MANIFEST_DIR").unwrap_or(".");
    std::path::Path::new(base)
        .join("..")
        .join("scripts")
        .join("tool-selection")
        .join("worker.py")
}
