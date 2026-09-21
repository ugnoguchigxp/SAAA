# World delivery route matrix

| Route | Body includes current World | Receipt matches body | Freshness boundary | Status |
| --- | --- | --- | --- | --- |
| OpenAI compatible | Yes, every HTTP generation | Yes | Immediately before each HTTP body | 7/7 offline wire verified |
| DynamicLan text | Yes, every HTTP generation | Yes | After allocation and semantic health probe | 7/7 offline wire verified |
| Shared LARM voice | Yes, every LLM generation | Yes | After LLM lease and ASR-confirmed scope | 7/7 offline wire verified |
| AgentSession | Yes; Tool continuation uses a fresh remote session and host-rebuilt history | Yes | Before each fresh session turn | 7/7 offline final-wire verified |
| reasoning MCP | Yes, reasoning-answer-v3 / world-evidence-v2 typed evidence | Yes, actual RPC arguments digest | After initialization, before tools/call | 6 wire cases verified; Tool continuation is contract N/A |
| Codex app-server / role SDK | Yes, five-element graph and runtime sources in `saaa.world-turn.v1` | Yes, actual turn/start or final JSONL digest | After thread creation, immediately before turn dispatch | 7/7 app-server transitions and role SDK observer verified offline |

Workspace-derived scope is activated only when `scopeRefs` is empty. Explicit scope references
remain authoritative and are never merged with another project by name or text similarity.

For exact current-state questions, the host may return a StateAnswer card before any provider
dispatch. The card includes `claim`, `source_ref`, `as_of`, and `status`; this is not evidence of
full live-provider acceptance.

## 2026-09-21 review evidence

`remaining-results.md` records the completed 42-cell offline matrix. The transitions are initial,
Tool continuation, provider fallback, Scope switch, correction, forget, and session resume. A
Scope change must stop before model wire; correction and forget must remove the old graph element;
every sent body must match its receipt digest. The reasoning MCP contract exposes no host Tool, so
its Tool-continuation cell is an explicit N/A rather than an unexecuted pass.

This matrix uses executable loopback fixtures and actual serializer/receipt boundaries. It does not
certify a live model, LAN daemon, shared voice service, UI interaction, audio playback, or TTFA.
