# Adaptive improvement progress

- AI-01: durable decision, outcome, override, artifact, activation, dirty-queue, dataset, and
  example tables added. Unknown/silent outcomes remain ineligible.
- AI-02: immediate, revisioned scope overrides with revoke support added.
- AI-03: dirty decisions materialize from a fixed upper event boundary in one transaction;
  post-boundary feedback remains queued for the next run.
- AI-04: the existing local learning window and idle gate now start the bounded SQLite
  materializer from app setup; background faults are ignored so they cannot interrupt a turn.
- AI-06/07: candidate → evaluated → shadow → eligible → active transitions require recorded
  gate inputs and a monotonic CAS policy revision. Candidate checks and invalidation fall back to
  rules.
- AI-06: a deterministic 10,000-resample paired runner now aggregates repeated steps by root
  lineage before calculating success/resource intervals, and rejects incomplete paired data
  instead of inferring unobserved outcomes.
- AI-08/09: provider/recipe dispatch and tool ranking now consume only enabled, active, matching
  adaptive policies after their existing hard filters. Multi-actor recipes are not sent through
  the one-actor runtime path; Tool ACL and invocation grants remain unchanged.
- AI-10: delegated work now chooses only among registered `read`, `test_run`, and `read_test`
  recipes that are subsets of the persisted delegation. The decision and dispatch claim commit
  together, and a Coding terminal event records the applicable verifier outcome against that
  exact Plan decision. Existing in-flight tasks are not re-planned.
- AI-11: terminal reports now retain their Goal-scoped delivery choice in the outbox. `both` is
  immediately displayable, while the only alternate eligible policy, `silent`, waits one
  schedule tick (45 seconds) and is delivered as a digest. Explicit `silent` or `speak`
  instructions remain fixed; meeting hold remains the final gate and never flushes early.
- AI-01/09: persisted Tool Selection decisions now also write a DecisionObservation with the
  actual selected eligible revision, policy revision, and selection mode in the same transaction.
- AI-08: role-routing receipt creation now resolves the same enabled adaptive Provider/recipe
  choice as dispatch and records its full eligible candidate set, selected recipe, mode, and
  policy revision in the shared DecisionObservation ledger within the receipt transaction.
- AI-12: personal-source forgetting invalidates adaptive datasets/artifacts/activations in the
  same writer transaction and returns dispatch to rules. This is conservative until every domain
  supplies complete source references.

A complete paired-evaluation runner and end-to-end real-adapter evaluation remain required.
They are intentionally not inferred from raw feedback.
