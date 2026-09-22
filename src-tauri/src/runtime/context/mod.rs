//! Common, generation-scoped context runtime.
//!
//! Phase 1 records every provider generation without persisting raw envelopes.
//! Scope resolution and source selection are added behind this boundary.

pub(crate) mod broker;
pub(crate) mod continuations;
pub(crate) mod generation;
pub(crate) mod generation_inputs;
pub(crate) mod health;
pub(crate) mod required;
pub(crate) mod role_projection;
pub(crate) mod schema;
pub(crate) mod segment;
pub(crate) mod scope;
pub(crate) mod source;
pub(crate) mod state_answer;
pub(crate) mod usage;
pub(crate) mod world;
