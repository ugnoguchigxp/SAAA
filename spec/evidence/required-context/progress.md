# Required Context progress

## RC-00

The common conversation path was rechecked on 2026-09-21. `conversation_inputs` loads a scope
snapshot and Personal State candidates in one read snapshot; `conversation_turn` sends the
resulting broker envelope through OpenAI-compatible, AgentSession, and DynamicLan providers.
Each provider creates a context generation before network dispatch. The existing schema records
the scope, policy, current instruction, and candidate source versions, and its dispatch validation
prevents a changed dependency from being sent.

## RC-01 through RC-04

`runtime/context/required.rs` now centralizes the pure classification of required, untrusted
context. Active, candidate, and disputed Personal State projections and pending original sources
are `Must`; unrelated references remain at their supplied requirement. The broker reserves the
space for those items before selection by evicting optional assistant/history blocks. It returns
`required_context_overflow` rather than omitting required material when that reservation cannot
fit.

## RC-06 through RC-10

Before each OpenAI-compatible request (including DynamicLan and shared-voice routes) and each
AgentSession round, the final serialized request value is checked for every selected `Must`
candidate. Missing material produces `required_context_missing_from_wire` before dispatch. Each
generation also records a digest-only `required-context-set` receipt alongside the existing wire
request digest and source-version inputs. Overflow, scope changes, and unavailable required
sources now produce distinct recovery text that asks the user to narrow scope, inspect the source,
or correct memory; no automatic deletion or provider escalation occurs.

The reasoning controller now uses the same required candidate set during `fit_context`: it evicts
only messages that do not contain a required item and returns `required_context_overflow` if the
remaining required state cannot fit. Its request is also wire-checked and records the same
digest-only required-set receipt before dispatch.

The implementation intentionally does not add a second persistent memory representation or alter
the public IPC/schema contract. Existing generation records continue to bind dispatch to source
versions, scope epochs, and policy revision.
