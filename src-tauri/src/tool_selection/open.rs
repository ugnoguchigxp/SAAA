use super::*;

/// Opens (or creates) a tool-selection database at `database_path` and builds the live service.
/// Used by the evaluation CLI and future management tooling. The seeded development fixture is
/// intentionally unavailable here: evaluators use `open_mock_service`, which supplies a blank
/// deterministic lane and records that it is mock rather than creating adaptive fixture history.
pub fn open_service(
    database_path: &std::path::Path,
    config: &ToolSelectionConfig,
) -> ToolSelectionResult<ToolSelectionService> {
    if config.mode == SelectionMode::Mock {
        return Err(ToolSelectionError::invalid());
    }
    let writer =
        crate::open_database_writer(database_path).map_err(|_| ToolSelectionError::storage())?;
    Ok(build_service(std::sync::Arc::new(writer), config, None))
}

/// Deterministic mock lane: hash embeddings and hash reranking. The evaluation CLI records
/// `lane=mock` so no one mistakes it for the real-model result.
pub fn open_mock_service(
    database_path: &std::path::Path,
) -> ToolSelectionResult<ToolSelectionService> {
    let writer =
        crate::open_database_writer(database_path).map_err(|_| ToolSelectionError::storage())?;
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
