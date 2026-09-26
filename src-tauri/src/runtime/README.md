# Runtime

The legacy normal-conversation response orchestrator was removed for a clean rebuild. Its former turn, role handoff, Butler, Memory/World context, and voice response code is not a template for the replacement.

The remaining runtime modules support coding, capabilities, history, generic cancellation, and shared infrastructure. Normal conversation currently returns an explicit unavailable error until the new Session runtime is connected.

Design direction: [response runtime rebuild proposal](../../../spec/docs/saaa-jarvis-response-runtime-rebuild-proposal.md) and [deletion plan](../../../spec/docs/saaa-jarvis-response-runtime-deletion-plan.md).
