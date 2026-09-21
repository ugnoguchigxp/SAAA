#![allow(dead_code)]
#[cfg(test)]
pub(crate) mod g1_tests;
pub(crate) mod inputs;
mod live;
pub(crate) mod question;
pub(crate) mod question_input;
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

#[cfg(test)]
mod g1_perf_tests;
