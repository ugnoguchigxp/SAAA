//! WorldFrame adapter.
#![allow(dead_code)]
pub(crate) mod render;
#[cfg(test)]
mod render_tests;
pub(crate) mod shadow;
#[cfg(test)]
mod shadow_boundary_tests;
#[cfg(test)]
mod shadow_perf_tests;
#[cfg(test)]
mod shadow_tests;
pub(crate) mod source;
#[cfg(test)]
mod source_tests;
pub(crate) mod turn;
#[cfg(test)]
mod turn_tests;
