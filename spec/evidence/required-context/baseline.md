# Required Context baseline

Recorded 2026-09-21 before RC implementation changes.

- Conversation requests enter through `runtime/conversation_turn.rs` and compose a common broker envelope.
- OpenAI-compatible, AgentSession, and DynamicLan paths receive the same selected and omitted candidate lists through `ModelStreamContext`.
- A generation manifest is created before dispatch and validates scope, policy, current instruction, and source dependencies at dispatch time.
- Before this change, active personal assertions were labelled `Should`, which allowed a full optional history to crowd them out.

The implementation keeps the existing schema and public IPC contract unchanged. Provider-specific wire wrappers remain subject to the existing generation request-size check.

The deterministic Japanese extraction corpus already contains 64 scenarios: eight each for
negation, correction, quotation, hypothesis, pending, local scope, multiple evidence, and tactical
boundaries. It exceeds the required 60 fixed cases and is retained as the natural-language
acceptance corpus rather than duplicated.
