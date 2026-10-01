# Runtime

The legacy normal-conversation response orchestrator was removed for a clean rebuild. Its former turn, role handoff, Butler, Memory/World context, and voice response code is not a template for the replacement.

The remaining runtime modules support coding, capabilities, history, generic cancellation, and shared infrastructure. The generic `start_turn` conversation route remains unavailable. The conversation screen uses `conversation_check/queue_runtime.rs`: it persists user input and runs Ornith's tool loop. During a final `answer` action, SSE content deltas are projected into provisional UI text and sentence chunks for TTS. The final answer is validated and saved once generation completes; successful streamed audio is not replayed by the speech queue.

For Ornith, `queue_context.rs` loads the bounded conversation and Personal State projection under the saved run scope. When `SAAA_MEMORY_ENABLED=1`, it also prepares a source-backed World frame. Both are untrusted evidence; only the saved current user message is the current instruction. The source projection and World dependencies are rechecked before accepting Ornith's result. The LARM `contextWindow` claim and the local provider budget bound every role request immediately before dispatch. Offered memory tools use the existing agent-tool dispatcher; public Web tools use the existing web-fetch host.

The old `.s11tnext/conversation-respond.txt` SystemContext belongs to the removed generic conversation executor. One conversation job now runs Ornith's reasoning, tool decisions, and final answer under one SystemContext. Tool results stay in evidence messages so they cannot become the current user instruction. Startup migrates unfinished legacy jobs into the conversation lane without starting the old model lanes.

Design direction: [five-provider concept](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5) and [runtime contract](../../../spec/docs/saaa-jarvis-five-provider-runtime-contract.md). These are target contracts, not claims that every behavior is implemented.
