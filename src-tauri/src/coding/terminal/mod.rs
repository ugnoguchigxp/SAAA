//! Dedicated CLI ownership, durable event ingress, and host verification.
mod automatic;
pub(crate) mod commands;
mod evidence;
mod host;
mod ingress;
mod launch;
pub(crate) mod ledger;
mod questions;
mod recovery;
mod retry;
#[cfg(test)]
mod tests;
mod verification_process;
mod verify;
pub(crate) use automatic::{automatic_answer, automatic_context};
pub(crate) use host::{spawn, start};
pub(crate) use launch::{probe, validate};
pub(crate) use questions::answer;

#[cfg(test)]
mod verification_tests;

#[cfg(test)]
mod integration_tests;
