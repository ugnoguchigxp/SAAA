# Required Context progress

## Card status (2026-09-21)

| Card | Status | Evidence / remaining condition |
| --- | --- | --- |
| RC-00 | Partially complete | Provider entry points and the 60 Japanese fixed cases are recorded. Configured-model execution has not been recorded. |
| RC-01 | Implemented and automated | `required.rs` classifies active/candidate/disputed state and excludes stale/scope-external state. |
| RC-02 | Implemented and automated | Corrections and pending source material remain required; stale assertion versions invalidate dispatch. |
| RC-03 | Implemented and automated | Broker admission prioritizes required material and returns overflow rather than dropping it. |
| RC-04 | Partially complete | Concrete initial adapter reserves and final-wire checks exist. Both OpenAI-compatible and AgentSession now retry an oversized Tool follow-up after removing only pre-turn optional history while validating every Must entry; per-model capability discovery remains. |
| RC-05 | Partially complete | Receipt/CAS, correction/forget/scope/task/delegation invalidation, and pre-Tool revalidation are tested. Chat Completions' HTTP loop and AgentSession's round adapter reject a Scope change that arrives while a provider finishes with the typed scope recovery code; equivalent live races remain. |
| RC-06 | Partially complete | OpenAI-compatible/DynamicLan initial delivery, final-wire validation, and optional-history retry for huge Tool-result follow-ups exist. Live provider acceptance remains. |
| RC-07 | Partially complete | DynamicLan and shared voice use the common validation path; live connection-acquisition/voice-switch race acceptance is pending. |
| RC-08 | Partially complete | AgentSession initial and Tool-round validation/revalidation exist; resumed external-session acceptance is pending. |
| RC-09 | Implemented and automated | Reasoning fitting preserves required entries and refuses required-only overflow. |
| RC-10 | Implemented, desktop acceptance pending | Typed recovery UI and actions exist; all three refusal states have not been exercised in the desktop UI. |
| RC-11 | Partially complete | Provider-settings migration and the additive Required Context receipt migration are automated. The 1/100/512-candidate allocation p95 is below 20ms on this host. Full scenario and configured real-model semantic evaluation are outstanding. |

## Current blockers

1. The native SAAA process is running, and its configured Provider Harness at
   `http://192.168.0.130:9810` is healthy and exposes the Agent Connection control API. The
   browser-served Vite page has no Tauri IPC, so it renders `処理先未選択` and cannot exercise the
   native Settings/Chat path. A native desktop automation surface (or a user-provided native
   interaction) is required for the remaining desktop refusal/recovery acceptance. No provider
   credentials or endpoint were changed.
2. `bun run check:local` is blocked at formatting by unrelated dirty files
   (`CodingConnectionFields.tsx` and `RoleRoutingSection.tsx`). `bun run typecheck` passes.
3. `bun run size:check` is blocked by shared ratchet changes and unregistered files across
   concurrent World/role-routing/steward work. Updating the common baseline would conceal
   unrelated changes, so it has intentionally not been changed.
4. RC-05's remaining adapter-level late-result races, RC-07/08's live transport races, and RC-11's
   configured-model semantic cases are implementation/acceptance work remaining in this
   repository; they are not external blockers.
5. A live Agent Connection create request was accepted and a prior test connection was released,
   but the selected model remained `pending` for more than the execution environment's 30-second
   command window. The authenticated connection list cannot be read without credentials, so no
   claim or inference result is recorded as acceptance evidence.

## RC-00

The common conversation path was rechecked on 2026-09-21. `conversation_inputs` loads a scope
snapshot and Personal State candidates in one read snapshot; `conversation_turn` sends the
resulting broker envelope through OpenAI-compatible, AgentSession, and DynamicLan providers.
Each provider creates a context generation before network dispatch. The existing schema records
the scope, policy, current instruction, and candidate source versions, and its dispatch validation
prevents a changed dependency from being sent.

The 60 Japanese fixed acceptance cases required by §6 are frozen in
[`japanese-fixed-cases.md`](japanese-fixed-cases.md). They are a human-authored evaluation
ledger for configured-model runs, not a claim that those model runs have already passed.

## Implemented portions: RC-01, RC-02, RC-03, RC-06--RC-09

`runtime/context/required.rs` now centralizes the pure classification of required, untrusted
context. Active, candidate, and disputed Personal State projections, pending original sources,
and compact references to active coding runs / delegated tasks are `Must`; unrelated references
remain at their supplied requirement. The broker reserves the
space for those items before selection by evicting optional assistant/history blocks. It returns
`required_context_overflow` rather than omitting required material when that reservation cannot
fit.

## Receipt and wire-delivery details

Before each OpenAI-compatible request (including DynamicLan and shared-voice routes) and each
AgentSession round, the final serialized request value is checked for every selected `Must`
candidate. Missing material produces `required_context_missing_from_wire` before dispatch.
`context_generations` now has additive `required_set_digest` and `scope_digest` columns. The
former is written before the dispatch CAS and binds the already-recorded final `request_digest` to
the ordered required candidate identities; the latter binds the receipt to the resolved scope.
Source versions, policy revision, and omission records stay normalized in
`context_generation_inputs`, and no context body is copied into the receipt.

The reasoning controller now uses the same required candidate set during `fit_context`: it evicts
only messages that do not contain a required item and returns `required_context_overflow` if the
remaining required state cannot fit. Its request is also wire-checked and records the same
digest-only required-set receipt before dispatch.

## Not implemented yet

- **RC-04 (partial):** each concrete provider attempt now applies a byte-denominated
  `ProviderInputBudget` before broker composition: OpenAI-compatible/DynamicLan acquire the
  provider session first and reserve the exact serialized initial tool offer plus JSON wrapper;
  AgentSession reserves the actual initial UI/coding capability-wrapper wire delta, including
  current coding context. A fallback is recomposed under its own budget.
  The obsolete generic pre-compose was removed, so admission is decided only under the concrete
  provider budget. Both OpenAI-compatible and
  AgentSession adapters now classify an exact final wire body over the 64 KiB conservative input
  allotment as `RequiredContextOverflow` whenever it contains Must context, including a Tool
  follow-up. On an oversized Tool follow-up, both adapters recompose up to twice by removing only
  pre-turn optional user/assistant history; every candidate removal is checked against the final
  required-wire verifier, and the current input plus Tool protocol suffix remain intact.
  Per-model capability discovery is still not implemented.
- **RC-05 (partial):** receipt columns and dispatch CAS are present. Real SQLite tests now cover
  Scope epoch changes and erased Personal State assertions both before dispatch and before late
  completion, plus explicit assertion correction, active coding-task completion, and delegated-
  task withdrawal before dispatch. OpenAI-compatible and AgentSession adapters revalidate the
  completed generation immediately before each host Tool action, refusing a changed Scope or
  unavailable required source before the Tool starts. Provider-level delayed-result acceptance
  coverage is still incomplete.
- **RC-10 (implemented, desktop acceptance pending):** recovery text and typed runtime failure
  codes distinguish overflow, Scope change, and unavailable required source. Chat treats these
  as dispatch refusals rather than automatic retries, and exposes controls to return the prior
  request for target narrowing, open Settings to inspect the retained original, or enter a new
  correction without deleting memory. A live desktop run that deliberately reaches each refusal
  is still pending.
- **RC-11:** full scenario, migration, p95, and configured-real-model acceptance have not been run.
