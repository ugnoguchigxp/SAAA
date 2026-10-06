//! Worker agent registry: profile revisions, approval, skills, mode, tasks and blocklist.
//!
//! Writes are user/host operations only (IPC in `commands`, builtin seeding at startup); the
//! conversation model has no path to them.
pub(crate) mod commands;
mod queries;
mod repository;
mod validate;

#[cfg(test)]
mod tests;

pub(crate) use queries::{
    get_agent, get_summary, get_web_search_mode, list_agents, list_blocklist, list_tasks,
    remove_blocklist,
};
pub(crate) use repository::{
    approve_revision, index_embedding, save_draft, save_skill, seed_builtin, set_enabled,
    set_web_search_mode,
};
