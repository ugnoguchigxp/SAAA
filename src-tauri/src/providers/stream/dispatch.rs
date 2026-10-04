//! Thin re-export facade so `providers::stream::*` keeps its historical path. The conversation
//! tool dispatch itself lives in `agent_dispatch`; this file only forwards the symbols.

#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use super::agent_dispatch::tool_was_offered;
pub(crate) use super::agent_dispatch::{available_agent_tools, execute_agent_tool};
pub(crate) use crate::generated_capabilities::tools::AgentToolOffer;
