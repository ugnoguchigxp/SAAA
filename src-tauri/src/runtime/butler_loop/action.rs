//! Provider output mapped onto the shared conversation action contract.
//! The runtime does not invent a spoken preface when the model omitted one.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFinish {
    Stop,
    ToolCalls,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish: ProviderFinish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    SpeakAndContinue {
        text: String,
    },
    UseTool {
        call: ToolCall,
        preface: Option<String>,
    },
    Answer {
        text: String,
    },
    AskAndWait {
        text: String,
    },
}

pub fn from_provider_turn(turn: ProviderTurn) -> Vec<AgentAction> {
    let text = turn
        .content
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    match turn.finish {
        ProviderFinish::Ask => vec![AgentAction::AskAndWait {
            text: text.unwrap_or_default(),
        }],
        ProviderFinish::Stop if turn.tool_calls.is_empty() => vec![AgentAction::Answer {
            text: text.unwrap_or_default(),
        }],
        ProviderFinish::Stop | ProviderFinish::ToolCalls => {
            let mut actions = Vec::new();
            if turn.tool_calls.is_empty() {
                if let Some(text) = text {
                    actions.push(AgentAction::SpeakAndContinue { text });
                }
                return actions;
            }
            let mut calls = turn.tool_calls.into_iter();
            if let Some(call) = calls.next() {
                actions.push(AgentAction::UseTool {
                    call,
                    preface: text,
                });
            }
            for call in calls {
                actions.push(AgentAction::UseTool {
                    call,
                    preface: None,
                });
            }
            actions
        }
    }
}
