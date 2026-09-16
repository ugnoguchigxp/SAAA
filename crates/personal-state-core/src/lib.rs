//! Deterministic Personal State contracts. No IO, model, clock or ID generation.
//! Adapters supply authenticated scopes, authoritative sources and adoption fences.
mod model;
mod reducer;
mod selection;

pub use model::*;
pub use reducer::*;
pub use selection::*;
