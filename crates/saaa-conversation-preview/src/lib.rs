pub(crate) mod config;
pub(crate) mod credential;
pub mod host;
pub(crate) mod isolation;
#[path = "../../../src-tauri/src/conversation_host/qwen.rs"]
pub(crate) mod qwen;
#[path = "../../../src-tauri/src/conversation_host/repository.rs"]
pub(crate) mod repository;
pub(crate) mod speech;

#[cfg(test)]
mod live_tests;
