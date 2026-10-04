use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
#[path = "typed_recall/typed_memory_type.rs"]
mod typed_memory_type;
#[cfg(test)]
pub use typed_memory_type::MAX_TYPED_RECALL_CALLS_PER_TURN;
#[cfg(test)]
pub(super) use typed_memory_type::RULE_ITEM_BYTES;
pub use typed_memory_type::{
    is_typed_recall_tool, parse_call_tool_result, parse_typed_recall_arguments,
    typed_recall_input_schema, typed_recall_tool_definitions, TypedMemoryType,
    TypedRecallContractError, ValidatedTypedRecallCall, MEMORY_RECALL_CONTRACT_VERSION,
    TYPED_RECALL_TOOL_NAMES,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub use typed_memory_type::{
    MAX_CALL_TOOL_RESULT_BYTES, RECALL_EXPERIENCE_TOOL_NAME, RECALL_RULE_TOOL_NAME,
    RECALL_SKILL_TOOL_NAME,
};
#[cfg(test)]
#[path = "typed_recall/tests.rs"]
mod tests;
