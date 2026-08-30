pub(crate) mod completion;
pub(crate) mod dynamic_lan;
#[allow(dead_code)]
pub(crate) mod larm;
pub(crate) mod openai_compatible;
pub(crate) mod probe;
mod probe_state;
pub(crate) mod routing;
pub(crate) mod session_store;
pub(crate) mod stream;

pub(crate) const DEFAULT_CONVERSATION_REASONING_EFFORT: &str = "medium";

pub(crate) fn default_conversation_reasoning_effort() -> String {
    DEFAULT_CONVERSATION_REASONING_EFFORT.to_string()
}

pub(crate) fn valid_conversation_reasoning_effort(value: &str) -> bool {
    matches!(value, "low" | "medium" | "xhigh")
}
