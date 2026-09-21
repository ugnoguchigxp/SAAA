# Required Context progress

## RC-00

The common conversation path was rechecked on 2026-09-21. `conversation_inputs` loads a scope
snapshot and Personal State candidates in one read snapshot; `conversation_turn` sends the
resulting broker envelope through OpenAI-compatible, AgentSession, and DynamicLan providers.
Each provider creates a context generation before network dispatch. The existing schema records
the scope, policy, current instruction, and candidate source versions, and its dispatch validation
prevents a changed dependency from being sent.

## Implemented portions: RC-01, RC-02, RC-03, RC-06--RC-09

`runtime/context/required.rs` now centralizes the pure classification of required, untrusted
context. Active, candidate, and disputed Personal State projections and pending original sources
are `Must`; unrelated references remain at their supplied requirement. The broker reserves the
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
  `ProviderInputBudget` before broker composition: OpenAI-compatible/DynamicLan reserve JSON
  wrapper and static-tool-schema space, and AgentSession reserves its own envelope space. A
  fallback is recomposed under its own budget. Per-model capability discovery and a second pass
  using the exact dynamically generated tool schema are still not implemented.
- **RC-05 (partial):** receipt columns and dispatch CAS are present. Real SQLite tests now cover
  Scope epoch changes and erased Personal State assertions both before dispatch and before late
  completion. Correction-source and delegation-revoke orderings, plus provider-wide prevention of
  late result/tool adoption, are still not proven.
- **RC-10 (partial):** recovery text and typed runtime failure codes now distinguish overflow,
  Scope change, and unavailable required source. The existing Chat failure display receives those
  values. Settings/Chat controls for narrowing a target, inspecting an original source, or
  correcting memory are not implemented yet.
- **RC-11:** full scenario, migration, p95, and configured-real-model acceptance have not been run.
