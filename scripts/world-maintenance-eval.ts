import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

// This runner uses synthetic SQLite databases and loopback test servers only.
// It never opens app_data or enables the production worker.
const phase = process.argv[process.argv.indexOf("--phase") + 1];
if (!/^P[1-6]$/.test(phase ?? ""))
  throw new Error("Usage: bun scripts/world-maintenance-eval.ts --phase P1..P6");
type Case = { id: string; phase: number; tests: string[]; dependency?: string; live?: string };
const cases: Case[] = [
  {
    id: "WM-C01",
    phase: 1,
    tests: [
      "world_maintenance_schema_reopen_preserves_preference_and_checkpoint",
      "unconfigured_contract_is_explicit_and_memory_off_preserves_tracking",
    ],
  },
  {
    id: "WM-C02",
    phase: 1,
    tests: [
      "world_maintenance_explicit_profile_never_falls_back_to_conversation",
      "world_maintenance_local_policy_rejects_cloud_forwarding_and_mismatched_allocation",
      "world_maintenance_backoff_is_bounded_and_recovers",
    ],
    live: "Local-only deployment and disconnected configured runtime",
  },
  {
    id: "WM-C03",
    phase: 1,
    tests: [
      "generation_forget_fences_before_late_results_and_keeps_materialization_separate",
      "expired_views_and_unconfirmed_cancellation_prevent_new_claims",
      "world_maintenance_foregrounds_share_reads_and_cancel_background_before_waiting",
    ],
    live: "ASR during TTS and foreground cancellation latency",
  },
  {
    id: "WM-C04",
    phase: 1,
    tests: [
      "world_maintenance_backoff_is_bounded_and_recovers",
      "wr_t09_finalized_user_reaches_existing_world_ledger",
    ],
  },
  {
    id: "WM-C05",
    phase: 1,
    tests: [
      "expired_world_stage_is_resumed_and_old_owner_cannot_ack",
      "world_maintenance_resumes_world_without_replaying_committed_continuity",
    ],
  },
  {
    id: "WM-C06",
    phase: 2,
    tests: [
      "world_maintenance_multisource_quotes_bind_versions_and_scope",
      "wr_t09_five_elements_commit_with_host_evidence_and_remain_hypotheses",
    ],
  },
  {
    id: "WM-C07",
    phase: 2,
    tests: [
      "wr_t09_five_elements_commit_with_host_evidence_and_remain_hypotheses",
      "rf5_g_03_deictic_resolution",
    ],
    live: "Local extractor handling quoted, denied and ambiguous natural statements",
  },
  {
    id: "WM-C08",
    phase: 2,
    tests: [
      "world_maintenance_resumes_world_without_replaying_committed_continuity",
      "world_maintenance_user_goal_uses_shared_objective_without_changing_continuity_scope",
      "d38_goal_retraction_removes_current_importance",
    ],
  },
  {
    id: "WM-C09",
    phase: 2,
    tests: [
      "world_maintenance_own_user_scope_commits_without_fake_project",
      "world_maintenance_multisource_quotes_bind_versions_and_scope",
      "t14_scope_isolation_and_authorization",
    ],
  },
  {
    id: "WM-C10",
    phase: 3,
    tests: [],
    dependency: "ContextStill immutable revision and original lineage contract",
  },
  {
    id: "WM-C11",
    phase: 3,
    tests: [],
    dependency: "ContextStill world_evidence_v1 eligibility integration",
  },
  {
    id: "WM-C12",
    phase: 3,
    tests: [],
    dependency: "ContextStill typed raw fetch, quote ranges and Scope proof",
  },
  {
    id: "WM-C13",
    phase: 4,
    tests: [
      "world_maintenance_quoted_outcome_updates_only_matching_counterexample",
      "d37_outcome_resend_is_a_noop_and_second_outcome_is_rejected",
      "wr_t10_correction_and_revoked_source_cannot_revive_old_fact",
    ],
  },
  {
    id: "WM-C14",
    phase: 2,
    tests: [
      "rf5_g_03_deictic_resolution",
      "t14_scope_isolation_and_authorization",
      "world_g1_05_ambiguous_seed_is_not_auto_selected",
      "world_g1_06_alias_resolves_only_when_it_is_unique",
    ],
    live: "Real LocalLLM same-name ambiguity corpus",
  },
  {
    id: "WM-C15",
    phase: 4,
    tests: [
      "edit_invalidates_captured_range_and_delete_blocks_resurrection",
      "v2_forgotten_source_version_is_not_revived_by_a_new_version_with_the_same_id",
      "m2_19_source_forget_invalidates_the_old_frame",
    ],
  },
  {
    id: "WM-C16",
    phase: 5,
    tests: [
      "rf5_g_01_four_intents_and_greeting_is_not_requested",
      "rf5_g_02_host_rejects_malformed_unknown_and_injected_scope",
      "d28_unknown_explicit_seed_yields_missing_knowledge_gap",
    ],
  },
  {
    id: "WM-C17",
    phase: 5,
    tests: ["one_claim_and_retry_budget_and_epoch_fence", "m2_26_impossible_budget_is_rejected"],
    live: "Disk-full fault injection",
  },
  {
    id: "WM-C18",
    phase: 1,
    tests: [
      "transient_outage_never_exhausts_malformed_attempt_budget",
      "world_maintenance_backoff_is_bounded_and_recovers",
    ],
    live: "Local model release change and reconnect",
  },
  {
    id: "WM-C19",
    phase: 2,
    tests: ["world_maintenance_multisource_quotes_bind_versions_and_scope"],
    live: "Real LocalLLM essential five-source and oversized-payload held corpus",
  },
  {
    id: "WM-C20",
    phase: 4,
    tests: [
      "world_maintenance_evidence_history_exceeds_payload_window_without_losing_dependencies",
      "unrelated_scope_input_does_not_abort_a_scoped_extraction_commit",
    ],
  },
  {
    id: "WM-C21",
    phase: 3,
    tests: [],
    dependency: "ContextStill receipt cursor and deletion feed contract",
  },
  {
    id: "WM-C22",
    phase: 4,
    tests: ["world_maintenance_quoted_outcome_updates_only_matching_counterexample"],
  },
  {
    id: "WM-C23",
    phase: 1,
    tests: [
      "second_process_is_rejected_before_database_open",
      "only_the_owner_process_runs_migration_backup_and_bootstrap",
      "crashed_owner_releases_lock_without_removing_the_lock_file",
    ],
  },
  {
    id: "WM-C24",
    phase: 1,
    tests: [
      "worker_persists_patch_coverage_and_result_in_one_commit_then_forget_erases",
      "real_db_backup_restore_merges_current_journal_and_missing_journal_blocks_open",
      "expired_world_stage_is_resumed_and_old_owner_cannot_ack",
    ],
    live: "Commit-completed journal-sync failure fault injection",
  },
  {
    id: "WM-C25",
    phase: 1,
    tests: [
      "world_maintenance_slow_local_generation_releases_writer_and_stale_lease_cannot_commit",
      "eight_readers_progress_while_the_writer_commits",
    ],
  },
  {
    id: "WM-C26",
    phase: 4,
    tests: [
      "world_maintenance_multisource_quotes_bind_versions_and_scope",
      "product_off_blocks_new_generation_but_keeps_cleanup",
    ],
  },
  {
    id: "WM-C27",
    phase: 1,
    tests: [
      "bounded_queue_refills_deferred_canonical_sources_without_loss",
      "newest_priority_preserves_one_oldest_turn_in_four",
    ],
    live: "Disk-full invalidation blocks World delivery",
  },
];
if (cases.length !== 27 || new Set(cases.map((c) => c.id)).size !== 27)
  throw new Error("Case registry incomplete or duplicated");
const directory = resolve("spec/evidence/world-maintenance");
await mkdir(directory, { recursive: true });
const reportPath = resolve(directory, "offline-report.json");
await writeFile(reportPath, JSON.stringify({ complete: false, phase, state: "running" }) + "\n");
const required = cases.filter((c) => c.phase <= Number(phase.slice(1)));
const passedNames = new Set<string>();
const errors: string[] = [];
for (const filter of [
  "memory::personal_state",
  "runtime::context::world",
  "persistence::sqlite::tests",
  "world_maintenance_diagnosis",
]) {
  const child = Bun.spawn(
    ["cargo", "test", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--lib", filter],
    { stdout: "pipe", stderr: "pipe" },
  );
  const [out, err] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
  ]);
  const exit = await child.exited;
  const matches = [...out.matchAll(/^test ([^\s]+) \.\.\. ok$/gm)];
  if (exit !== 0 || matches.length === 0) {
    errors.push(`${filter}: exit=${exit}, passed=${matches.length}`);
    process.stderr.write(err.slice(-4000) + out.slice(-4000));
  }
  for (const match of matches) passedNames.add(match[1]!);
  console.log(`${filter}: ${matches.length} passed, exit=${exit}`);
}
const results = cases.map((c) => {
  const missing = c.tests.filter(
    (name) => [...passedNames].filter((full) => full.endsWith(`::${name}`)).length !== 1,
  );
  return {
    ...c,
    required: required.includes(c),
    state: c.dependency
      ? "blocked_dependency"
      : missing.length
        ? "missing_or_failed"
        : "offline_assertions_passed",
    missing,
  };
});
const offlineComplete =
  errors.length === 0 &&
  results.filter((c) => c.required).every((c) => c.state === "offline_assertions_passed");
// Live qualifications are explicit remaining conditions, never satisfied by mock tests.
const phaseOfflineAccepted = offlineComplete && Number(phase.slice(1)) < 6;
const complete = false;
const report = {
  schemaVersion: 1,
  phase,
  complete,
  offlineComplete,
  phaseOfflineAccepted,
  liveComplete: false,
  errors,
  cases: results,
  remainingLive: results
    .filter((c) => c.required && c.live)
    .map((c) => ({ id: c.id, condition: c.live })),
  longRunningAcceptance: "24-hour real LocalLLM and voice acceptance not run",
};
await writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
console.log(
  `World maintenance ${phase}: offline=${offlineComplete}, full=${complete}; ${reportPath}`,
);
if (!phaseOfflineAccepted) process.exitCode = 1;
