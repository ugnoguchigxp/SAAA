# Adaptive improvement completion audit

Recorded: 2026-09-21. This is an implementation-and-evidence audit, not a claim of
real-user improvement. Synthetic fixtures exercise the same local selection and execution
paths, but do not satisfy the held-out, real-adapter comparison required for final acceptance.

| Card | Current implementation evidence | Verification status | Remaining acceptance condition |
| --- | --- | --- | --- |
| AI-00 | [baseline.md](baseline.md) records the rules fallback and four-domain scope. | Partial | A pre-registered, unused evaluation pool does not exist. |
| AI-01 | `adaptive_improvement` migration stores decisions, outcomes, sources, datasets, artifacts, and activations. | Implemented | No real-history migration corpus exists to validate at scale. |
| AI-02 | Revisioned scope override selection is implemented. | Unit-tested: `ai_02_scope_override_changes_only_next_matching_choice`. | None for the local contract. |
| AI-03 | Fixed-boundary dirty materialization and cursor advancement are transactional. | Unit-tested: `ai_03_materialization_snapshots_outcomes_and_keeps_silence_unknown`. | Crash/restart evaluation over a real event stream remains unobserved. |
| AI-04 | App setup starts the bounded worker; Settings can run the local pass. | Implemented | Foreground cancellation and restart timing need a live evaluation run. |
| AI-05 | Deterministic aggregate artifacts only label the selected, observed candidate. | Unit-tested: `ai_05_trainer_scores_only_observed_selected_candidates`, `ai_06_insufficient_data_cannot_be_promoted`. | No production-sized dataset exists. |
| AI-06 | Grouped 10,000-resample paired evaluation is available. | Unit-tested: grouped/reproducible bootstrap and incomplete-pair rejection. | No held-out paired task pool, so no qualifying interval exists. |
| AI-07 | Gate, CAS activation, dispatch fingerprint recheck, and rollback are implemented. | Unit-tested: eligible-only activation, fallback, and rollback. | Requires an evaluated artifact from a qualifying pool. |
| AI-08 | Provider/recipe dispatch records the chosen adaptive decision and terminal outcome. | Unit-tested: `ai_08_provider_terminal_result_is_recorded_for_the_dispatch_decision`. | Live-provider paired comparison remains required. |
| AI-09 | Tool hard filters, grants, search, invocation, and outcome recording remain intact under adaptive ranking. The developer mock uses an isolated database. | Unit-tested: three mock tests and `ai_09_synthetic_approved_artifact_reorders_the_real_tool_search_path`. | Synthetic success is not real-user improvement evidence. |
| AI-10 | Delegated work selects only registered recipe subsets and records verifier outcomes. | Unit-tested: `registered_plan_recipes_are_strict_subsets_of_delegated_ops`. | Same-Goal before/after verifier comparison is missing. |
| AI-11 | Delivery policy is recorded, hold/deadline gates are retained, and delivered outcomes are recorded. | Unit-tested: deadline, restart-claim, and unique terminal-delivery tests. | Real notification usefulness and deadline comparison is missing. |
| AI-12 | Source forgetting and candidate-version mismatch invalidate adaptive applicability. | Unit-tested: source invalidation and candidate-version fallback. | Real deletion/retraining corpus is missing. |
| AI-13 | Settings expose artifact status and a return-to-rules action. | Unit-tested: `ai_13_rollback_active_artifact_returns_matching_scope_to_rules` and Settings snapshot test. | UI usability assessment remains open. |
| AI-14 | No final four-domain improvement assertion has been made. | Not complete by design | Requires the unconsumed paired pool, actual adapter results, performance measurements, and the §6 acceptance scenarios. |

## Synthetic Tool fixture boundary

`bun run start:adaptive-fixture` is a developer-only run: it requires the explicit fixture flag,
smoke marker, and an existing private `saaa-adaptive-fixture.*` data directory. It has three
fixture Tools with deliberately known outcomes and may activate only its isolated Tool artifact.
The normal application configuration cannot seed the fixture rows merely by pointing at the mock
configuration file. The generic service opener also rejects the seeded mock configuration; the
evaluation CLI uses an explicitly blank mock lane instead.

## Current external blocker

The final AI-14 acceptance cannot be derived from the synthetic fixture. It requires user-approved
evaluation cases and, for Provider/recipe, actual provider executions. Until those exist, the
implementation can be tested for safety and wiring but must remain reported as unproven for
real-world improvement.
