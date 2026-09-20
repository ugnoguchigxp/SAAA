# World delivery route matrix

| Route | Body includes current World | Receipt matches body | Freshness boundary | Status |
| --- | --- | --- | --- | --- |
| OpenAI compatible | Yes | Yes | Per HTTP request | Existing |
| DynamicLan text | Yes | Yes | After allocation, in shared HTTP adapter | Implemented |
| Shared LARM voice | Yes | Yes | After LLM lease, in shared HTTP adapter | Implemented |
| AgentSession | Yes, initial turn; no World in tool follow-ups | Yes | After session/transport acquisition | Implemented |
| reasoning MCP | Yes, typed evidence | Yes | Before MCP request construction | Implemented |
| Codex CLI/SDK | Scope and Task snapshot in developer context | No receipt yet | Before fresh thread start | Partial |

Workspace-derived scope is activated only when `scopeRefs` is empty. Explicit scope references
remain authoritative and are never merged with another project by name or text similarity.

For exact current-state questions, the host may return a StateAnswer card before any provider
dispatch. The card includes `claim`, `source_ref`, `as_of`, and `status`; this is not evidence of
full live-provider acceptance.
