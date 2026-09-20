//! Backend assembly for the D4 build path. Kept out of `tool_selection::mod` so the base module
//! stays close to its pre-D4 size.

use std::sync::Arc;

use super::super::backends::llang::LlangBackend;
use super::super::backends::mcp::McpBackend;
use super::super::backends::router::BackendRouter;
use super::super::backends::ToolBackend;
use super::super::contracts::ToolSelectionConfig;
use super::super::inference::EmbeddingProvider;
use super::super::service::ToolSelectionService;
use super::config::McpSources;
use super::manager::McpManager;
use crate::generated_capabilities::service::CapabilityService;
use crate::persistence::SqliteWriter;

/// Builds the backend router and, when configured, the external-MCP manager. A source file that
/// fails to parse on first load starts MCP disabled with a diagnostic instead of partially
/// applying the file.
pub fn assemble(
    writer: Arc<SqliteWriter>,
    config: &ToolSelectionConfig,
    capabilities: Option<Arc<CapabilityService>>,
    embedding: Arc<dyn EmbeddingProvider>,
) -> (Arc<dyn ToolBackend>, Option<Arc<McpManager>>) {
    let llang: Arc<dyn ToolBackend> = Arc::new(LlangBackend::new(capabilities));
    let manager = build_manager(writer, config, embedding);
    let backend: Arc<dyn ToolBackend> = match &manager {
        Some(manager) => Arc::new(BackendRouter::new(
            llang,
            Arc::new(McpBackend::new(manager.clone())),
        )),
        None => llang,
    };
    (backend, manager)
}

/// Attaches an existing manager to a service, used by tests and management callers.
pub fn attach(service: &mut ToolSelectionService, manager: Option<Arc<McpManager>>) {
    if let Some(manager) = manager {
        service.set_mcp_manager(manager);
    }
}

fn build_manager(
    writer: Arc<SqliteWriter>,
    config: &ToolSelectionConfig,
    embedding: Arc<dyn EmbeddingProvider>,
) -> Option<Arc<McpManager>> {
    let path = config.mcp_sources_path.clone()?;
    let principal = match super::super::service::ensure_principal(&writer) {
        Ok(principal) => principal,
        Err(_) => return None,
    };
    let (sources, diagnostic) = match McpSources::load(&path) {
        Ok(sources) => (sources, None),
        Err(code) => (McpSources::default(), Some(code)),
    };
    Some(McpManager::new(
        writer,
        principal,
        Some(path),
        sources,
        Some(embedding),
        diagnostic,
    ))
}
