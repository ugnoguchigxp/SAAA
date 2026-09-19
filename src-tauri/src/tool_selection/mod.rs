//! Tool-selection D0–D3: a SQLite-backed ledger of L-Lang capabilities, hybrid retrieval with a
//! local ML worker, conditional user-correction memory, and the three conversation entry points.
//! MCP transport and ranking learning are explicitly out of scope for this milestone.

pub mod backends;
pub mod catalog;
pub mod contracts;
pub mod extraction;
pub mod feedback;
pub mod inference;
pub mod ranking;
pub mod references;
pub mod repository;
pub mod retrieval;
pub mod rules;
pub mod schema;
pub mod service;
pub mod worker;

#[cfg(test)]
mod tests;

pub use contracts::{
    RequestContext, Scenario, SelectionMode, ToolSelectionConfig, ToolSelectionError,
    ToolSelectionErrorCode, ToolSelectionResult,
};
pub use service::ToolSelectionService;
