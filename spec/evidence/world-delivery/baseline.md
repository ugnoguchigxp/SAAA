# World delivery baseline

Recorded: 2026-09-21

| Route | Wire adapter | World handling before this change |
| --- | --- | --- |
| OpenAI compatible | `providers/chat_completions` | Revalidated immediately before every request and tool follow-up. |
| DynamicLan text | OpenAI-compatible adapter after allocation | The turn layer removed the World block before dispatch. |
| Shared LARM voice | OpenAI-compatible adapter after lease acquisition | The turn layer removed the World block before dispatch. |
| AgentSession | `providers/agent_session/sse` | The turn layer removed the World block before creating the remote turn. |
| reasoning MCP | reasoning controller | No typed World evidence contract. |

The current implementation has no separately registered UI scope chooser. `StartTurnInput.scopeRefs`
is the existing explicit scope interface and is validated by `runtime/context/scope.rs`.
