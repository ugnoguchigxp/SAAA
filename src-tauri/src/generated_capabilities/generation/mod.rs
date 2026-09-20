//! Restricted L-Lang generation flow: host-registered requests → one model call → fixed CLI
//! build/inspect → M1 import/verify → publication. The model never supplies paths, commands,
//! credentials or its own tests.

pub mod builder;
pub mod config;
pub mod contracts;
pub mod generator;
pub mod kit;
pub mod recovery;
pub mod repository;
pub mod schema;
pub mod service;
