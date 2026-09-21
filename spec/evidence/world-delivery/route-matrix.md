# World delivery route matrix

| Route | Body includes current World | Receipt matches body | Freshness boundary | Status |
| --- | --- | --- | --- | --- |
| OpenAI compatible | Yes | Yes | Per HTTP request | Existing |
| DynamicLan text | Yes | Yes | After allocation, in shared HTTP adapter | Implemented |
| Shared LARM voice | Yes | Yes | After LLM lease, in shared HTTP adapter | Implemented |
| AgentSession | Yes, initial turn; no World in tool follow-ups | Yes | After session/transport acquisition | Implemented |
| reasoning MCP | Yes, reasoning-answer-v2 typed evidence | Yes, actual RPC arguments digest | After initialization, before dispatch | Offline verified |
| Codex CLI/SDK | Scope/Task/deadline metadata in developer context; no five-element graph | Yes, thread/start + turn/start hashes | Before fresh thread start, before dispatch and at result acceptance | Metadata route offline verified; full graph/live pending |

Workspace-derived scope is activated only when `scopeRefs` is empty. Explicit scope references
remain authoritative and are never merged with another project by name or text similarity.

For exact current-state questions, the host may return a StateAnswer card before any provider
dispatch. The card includes `claim`, `source_ref`, `as_of`, and `status`; this is not evidence of
full live-provider acceptance.

## 2026-09-21 review evidence

`../world-model/m4a-results.md` records offline HTTP acceptance for OpenAI-compatible,
AgentSession initial/follow-up/expired fallback, and reasoning MCP current/expired requests.
The MCP adapter now removes the duplicate World history block and records only evidence actually
adopted into the bounded request. This does not certify live DynamicLan allocation, shared voice,
or the full live-provider/UI matrix. Codex receipt verification was added during resumed review; it uses an executable protocol fixture, not a live Codex model. The M4A/G1 legacy AgentSession exclusion has
been replaced with this table's initial-only contract.
