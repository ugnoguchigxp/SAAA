# Runtime

The legacy normal-conversation response orchestrator was removed for a clean rebuild. Its former turn, role handoff, Butler, Memory/World context, and voice response code is not a template for the replacement.

The remaining runtime modules support coding, capabilities, history, generic cancellation, and shared infrastructure. The generic `start_turn` conversation route remains unavailable. The conversation screen uses `conversation_check/queue_runtime.rs`: it persists user input, routes Qwen decisions to Ornith, and commits quick Qwen replies or Ornith's final answer unchanged before speech.

For Ornith, `queue_context.rs` loads the bounded conversation and Personal State projection under the saved run scope. When `SAAA_MEMORY_ENABLED=1`, it also prepares a source-backed World frame. Both are untrusted evidence; only the saved current user message is the current instruction. The source projection and World dependencies are rechecked before accepting Ornith's result. The LARM `contextWindow` claim and the local provider budget bound every role request immediately before dispatch. Offered memory tools use the existing agent-tool dispatcher; public Web tools use the existing web-fetch host.

The old `.s11tnext/conversation-respond.txt` SystemContext belongs to the removed generic conversation executor. The queue has separate SystemContext instructions for Qwen control and Ornith investigation; its result and tool data stay in evidence messages so they cannot become the current user instruction.

Design direction: [five-provider concept](../../../spec/docs/saaa-jarvis-five-provider-concept.md) and [runtime contract](../../../spec/docs/saaa-jarvis-five-provider-runtime-contract.md). These are target contracts, not claims that every behavior is implemented.
