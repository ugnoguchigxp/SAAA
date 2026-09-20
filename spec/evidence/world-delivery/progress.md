# World delivery progress

## Implemented

- DynamicLan and shared LARM voice now receive the composed history and the existing
  OpenAI-compatible adapter performs its post-allocation/post-lease World freshness check.
- AgentSession now revalidates and renders the World only after remote-session creation and
  transport negotiation, immediately before its first remote turn.
- AgentSession generation receipts bind the sent World frame and only record the World source
  when the initial wire body contains it. Tool follow-ups do not claim a stale frame was resent.
- AgentSession tool follow-ups now use a separately rendered World-free base conversation, so the
  remote session cannot receive the initial World snapshot again inside its continuation envelope.
- reasoning MCP now receives selected World data as bounded typed evidence, and performs a
  freshness check at its request boundary before recording the dispatch receipt.
- Coding workspace selection now creates a durable Project → resource link. With no competing
  explicit scope, normal conversation resolves that registered Project and its active coding Task.
- Chat now sends the selected coding Project/resource and active Task as explicit `scopeRefs`, so
  the normal UI route does not depend on backend defaults or text similarity.
- Narrow current-state questions return a host-verified `StateAnswer` card with a source revision
  and observation time. Current Task uses `coding_jobs`; next deadline uses `schedule_entries`;
  an unobservable meeting is explicitly `unknown`.
- Voice acknowledgements re-check the Situation hold both before synthesis and before queueing, so
  a meeting detected after ASR cannot receive a stale acknowledgement.
- Codex read-only turns now start a fresh remote decision thread per generation. A bounded,
  data-only Scope/Task snapshot is placed in the developer-instruction boundary, preventing a
  resumed external thread from reviving a previous World body.
- World omission now produces a stable `world-context-omitted` runtime activity reason without
  including the context body in diagnostics.
- Normal provider attempts now re-read and compose the source-backed context after provider
  session acquisition. The initial preflight composition is not used for their wire body; each
  fallback attempt has one bounded fresh composition.
- Regression tests cover current and stale provider-history rendering.

## Deliberately not claimed complete

The reasoning contract is still on its existing generic `Evidence` schema rather than a separately
versioned World-evidence protocol. There is also no live-provider/UI acceptance matrix yet. Those
conditions remain unverified; this evidence does not claim WD-10 through WD-13 complete.
