//! Restricted L-Lang generation flow: registered request → one model call → fixed CLI
//! build/inspect → import/verify → publication. The model supplies no paths or commands.

pub mod builder;
pub mod config;
pub mod contracts;
pub mod generator;
pub mod kit;
pub mod packager;
pub mod projection;
pub mod recovery;
pub mod repository;
pub mod schema;
pub mod service;
