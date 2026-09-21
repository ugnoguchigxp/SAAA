# Adaptive improvement progress

- AI-01: durable decision, outcome, override, artifact, activation, dirty-queue, dataset, and
  example tables added. Unknown/silent outcomes remain ineligible.
- AI-02: immediate, revisioned scope overrides with revoke support added.
- AI-03: dirty decisions materialize from a fixed upper event boundary in one transaction;
  post-boundary feedback remains queued for the next run.
- AI-04: the existing local learning window and idle gate now start the bounded SQLite
  materializer from app setup; background faults are ignored so they cannot interrupt a turn.
- AI-05: each ready immutable dataset is grouped by domain, scope, and exact eligible-candidate
  fingerprint to create deterministic aggregate candidate artifacts. Only the candidate that was
  actually selected receives an explicit technical, verifier, or user-acceptance label; an
  unselected candidate is never made a negative example. Artifacts retain the dataset event
  boundary and remain `candidate` pending evaluation.
- AI-04/05: the existing Settings action for a local learning pass now materializes and trains
  the adaptive ledger too when its user-approved domains are enabled. It remains database-only:
  it neither activates a policy nor contacts a provider.
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
- AI-08/09/11: Provider completion, the actually policy-selected Tool invocation, and a
  notification that is actually written to the conversation now record their technical outcomes
  in the same relevant persistence path. A manually invoked lower-ranked Tool is deliberately
  not attributed to the ranker's top choice; delayed or held reports receive no outcome before
  delivery.
- AI-12: personal-source forgetting invalidates adaptive datasets/artifacts/activations in the
  same writer transaction and returns dispatch to rules. This is conservative until every domain
  supplies complete source references. A candidate-set fingerprint change also returns only that
  dispatch to rules without mutating the still-auditable artifact.
- AI-13: Settings now lists each active or pending adaptive artifact in plain language, including
  its domain, Scope, eligible-result count, best observed result, and policy revision. An active
  artifact can be returned to fixed rules from the same screen; this retires only that learned
  policy and leaves explicit user overrides intact.
- AI-09: an isolated synthetic fixture now creates an authorized three-Tool catalog, passes a
  controlled gate, activates the resulting Tool artifact, and verifies that the normal `search`
  path selects the learned Tool, invokes it through the regular execution reference, and records
  both `selection_mode=adaptive` and its successful outcome. The fixture is in-memory only and
  cannot create user history or production evidence.
- AI-09: an explicit `mode: "mock"` Tool Selection configuration now enables the same three
  Tool definitions in a running development build without a local model, external MCP source,
  or external invocation. It seeds only `adaptive-development-fixture` catalog rows and three
  explicit synthetic outcomes (web/archive failure, minutes success), materializes them through
  the normal dataset path, trains an active Tool artifact that prefers `minutes`, and uses the
  deterministic fixture backend. Direct and real discovery modes never receive those rows.
  `bun run start:adaptive-fixture` supplies that explicit configuration and an isolated temporary
  DB for a development launch. The runtime additionally requires
  `SAAA_ADAPTIVE_FIXTURE=1`, a smoke marker, and an absolute smoke-data directory; a mock
  configuration file alone is disabled before any fixture catalog, observation, outcome, or
  artifact is seeded into the application ledger. Fixture-mode database resolution also accepts
  only an existing private `saaa-adaptive-fixture.*` launcher directory that differs from normal
  app data. The launcher removes that temporary directory at exit unless
  `SAAA_ADAPTIVE_FIXTURE_KEEP_DATA=1` is explicitly set for investigation. The generic
  Tool-selection service opener refuses the seeded mock configuration; the evaluator uses its
  separate blank mock lane instead.

An automatic held-out split and paired evaluation against a fixed rules baseline, plus
end-to-end real-adapter evaluation, remain required. They are intentionally not inferred from
raw feedback or the controlled fixture.
