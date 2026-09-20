//! Common, generation-scoped context runtime.
//!
//! Phase 1 records every provider generation without persisting raw envelopes.
//! Scope resolution and source selection are added behind this boundary.

pub(crate) mod broker;
pub(crate) mod generation;
pub(crate) mod generation_inputs;
pub(crate) mod health;
pub(crate) mod schema;
pub(crate) mod scope;
pub(crate) mod source;
pub(crate) mod world_render;
pub(crate) mod world_shadow;
pub(crate) mod world_source;

#[cfg(test)]
mod world_render_tests;
#[cfg(test)]
mod world_shadow_boundary_tests;
#[cfg(test)]
mod world_shadow_perf_tests;
#[cfg(test)]
mod world_shadow_tests;
#[cfg(test)]
mod world_source_tests;
