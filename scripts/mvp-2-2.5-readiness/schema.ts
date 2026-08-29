import { z } from "zod";

export const SCHEMA_VERSION = "saaa-mvp-2-2.5-readiness-v1" as const;

export const REASON_CODES = [
  "environment-invalid",
  "operator-blocked",
  "permission-lifecycle-failed",
  "resource-cleanup-failed",
  "asr-contract-failed",
  "persistence-contract-failed",
  "privacy-contract-failed",
  "agent-policy-failed",
  "threshold-exceeded",
  "process-exited",
  "observation-invalid",
] as const;

export const RESULT_VALUES = ["pass", "fail", "blocked"] as const;
export const BUILD_CLASSES = ["development", "signed-packaged"] as const;
export const SUITES = ["meeting", "input-activity", "agent-run"] as const;
export const MODES = ["functional", "soak-30m", "soak-2h", "manual"] as const;

export type Suite = typeof SUITES[number];
export type Mode = typeof MODES[number];
export type BuildClass = typeof BUILD_CLASSES[number];
export type Result = typeof RESULT_VALUES[number];
export type ReasonCode = typeof REASON_CODES[number];

export interface MetricSpec {
  key: string;
  unit: "count" | "milliseconds" | "seconds" | "mib";
  description: string;
  min?: number;
  max?: number;
  exact?: number;
  automatic?: "rss-median-delta" | "elapsed-seconds" | "workspace-integrity";
}

export interface CaseSpec {
  caseId: string;
  buildClass: BuildClass;
  instruction: string;
  metrics: MetricSpec[];
}

export const count = (key: string, description: string, exact = 0): MetricSpec => ({ key, unit: "count", description, exact });
export const atLeast = (key: string, description: string, min = 1): MetricSpec => ({ key, unit: "count", description, min });
export const latency = (key: string, description: string, max = 5_000): MetricSpec => ({ key, unit: "milliseconds", description, max });

export function forBuilds(caseId: string, instruction: string, metrics: MetricSpec[]): CaseSpec[] {
  return BUILD_CLASSES.map((buildClass) => ({ caseId, buildClass, instruction, metrics }));
}

export const MEETING_FUNCTIONAL_CASES: CaseSpec[] = [
  ...forBuilds("permission-grant", "未決定状態からmicrophoneを許可し、capture開始とtrack解放を確認する。", [atLeast("captureStartCount", "capture開始回数"), latency("trackReleaseMs", "track解放時間")]),
  ...forBuilds("permission-deny", "microphoneを拒否し、capture 0とrecovery表示を確認する。", [count("captureStartCount", "capture開始回数"), atLeast("recoveryVisibleCount", "recovery表示回数")]),
  ...forBuilds("permission-loss", "active中にpermission revokeまたはdevice lossを発生させ、安全遷移とcleanupを確認する。", [count("captureResourceRemainingCount", "残存capture resource数"), count("asrTaskRemainingCount", "残存ASR task数"), latency("indicatorReleaseMs", "microphone indicator消灯時間")]),
  ...forBuilds("partial-final", "real LAN ASRで同一lane/sequenceのPartialがFinalへ置換されることを確認する。", [atLeast("partialFinalReplacementCount", "Partial→Final置換回数"), count("sequenceViolationCount", "sequence重複・逆行回数")]),
  ...forBuilds("pause", "5分pauseでtranscriptが増えずindicatorが消灯することを確認する。", [count("transcriptGrowthDuringPauseCount", "pause中の追加entry数"), latency("indicatorReleaseMs", "microphone indicator消灯時間")]),
  ...forBuilds("resume", "resumeで新capture tokenを使いsequence違反がないことを確認する。", [count("captureTokenReuseCount", "capture token再利用回数"), count("sequenceViolationCount", "sequence重複・逆行回数")]),
  ...forBuilds("stop-idempotent", "Stopを連打しCompleted snapshotが一つだけになることを確認する。", [{ key: "completedSnapshotCount", unit: "count", description: "Completed snapshot数", exact: 1 }, latency("indicatorReleaseMs", "microphone indicator消灯時間")]),
  ...forBuilds("tts-guard", "Meeting active/paused中のTTS開始がなく、既存再生も停止することを確認する。", [count("ttsStartCount", "Meeting中のTTS開始回数"), count("activeTtsRemainingCount", "Meeting開始後の残存TTS数")]),
  ...forBuilds("app-close", "Surface移動、unmount、通常close後にcapture/ASR/childが残らないことを確認する。", [count("captureResourceRemainingCount", "残存capture resource数"), count("asrTaskRemainingCount", "残存ASR task数"), count("childProcessRemainingCount", "残存child process数")]),
  { caseId: "save-review", buildClass: "signed-packaged", instruction: "Save reviewのtarget、Final件数、language、raw audio非保存を確認する。", metrics: [{ key: "reviewMatchesCount", unit: "count", description: "reviewと実測が一致した項目", exact: 4 }, count("rawAudioFileCount", "raw audio file数")] },
  { caseId: "save-final-only", buildClass: "signed-packaged", instruction: "Save前後・reopen後のDB rowとFinal/language状態を確認する。", metrics: [count("preSaveTranscriptRowCount", "Save前transcript row数"), count("nonFinalPersistedCount", "保存された非Final数"), { key: "savedRowCountMismatch", unit: "count", description: "UI reviewとDB件数の不一致", exact: 0 }, count("languageStateMismatch", "original_language状態の不一致")] },
  { caseId: "discard", buildClass: "signed-packaged", instruction: "Discard後にtranscript bodyが残らないことを確認する。", metrics: [count("transcriptRowCount", "Discard後transcript row数")] },
  { caseId: "close-without-save", buildClass: "signed-packaged", instruction: "Saveしないapp close後にtranscript bodyが残らないことを確認する。", metrics: [count("transcriptRowCount", "reopen後transcript row数")] },
];

export const INPUT_ACTIVITY_CASES: CaseSpec[] = [
  ...forBuilds("monitoring-off", "monitoring offで10秒間OS input API hitが0件であることをsymbolic breakpointで確認する。", [count("osApiHitCount", "OS input API hit数")]),
  ...forBuilds("bounded-payload", "monitoring onでUI/contractにcategoryとhealthだけが現れることを確認する。", [count("rawDurationPayloadCount", "raw duration payload数"), count("lastInputTimestampPayloadCount", "last-input timestamp payload数")]),
  ...forBuilds("keyboard-reset", "keyboard操作後30秒以内にactiveへ戻ることを確認する。", [latency("activeResetLatencyMs", "activeへのreset時間", 30_000)]),
  ...forBuilds("mouse-reset", "mouse操作後30秒以内にactiveへ戻ることを確認する。", [latency("activeResetLatencyMs", "activeへのreset時間", 30_000)]),
  ...forBuilds("trackpad-reset", "trackpad操作後30秒以内にactiveへ戻ることを確認する。", [latency("activeResetLatencyMs", "activeへのreset時間", 30_000)]),
  ...forBuilds("recent-transition", "30秒超から5分以内にrecentへ遷移することを確認する。", [{ key: "transitionSeconds", unit: "seconds", description: "recent遷移までの秒数", min: 31, max: 300 }]),
  ...forBuilds("idle-transition", "5分超でidleへ遷移することを確認する。", [{ key: "transitionSeconds", unit: "seconds", description: "idle遷移までの秒数", min: 301, max: 900 }]),
  ...forBuilds("lock-unlock", "screen lock/unlock後にbounded categoryまたはdegraded fallbackへ戻ることを確認する。", [count("recoveryFailureCount", "recovery失敗回数"), atLeast("boundedStateCount", "確認したbounded state数")]),
  ...forBuilds("sleep-resume", "sleep/resume後にpanicせずbounded categoryまたはdegraded fallbackへ戻ることを確認する。", [count("recoveryFailureCount", "recovery失敗回数"), atLeast("boundedStateCount", "確認したbounded state数")]),
  ...forBuilds("permission-prompts", "Accessibility/Input Monitoring permission promptが0件であることを確認する。", [count("permissionPromptCount", "permission prompt数")]),
  { caseId: "sampling-soak", buildClass: "signed-packaged", instruction: "30分sampling soakのsample数とRSS増加を確認する。", metrics: [{ key: "elapsedSeconds", unit: "seconds", description: "sampling継続時間", min: 1_800, automatic: "elapsed-seconds" }, { key: "sampleCount", unit: "count", description: "sampling件数", min: 890 }, { key: "rssGrowthMiB", unit: "mib", description: "RSS中央値増加量", max: 32, automatic: "rss-median-delta" }] },
  ...forBuilds("privacy-scan", "SQLite、diagnostics、payload、frontend、console、reportのprivacy leakが0件であることを確認する。", [count("rawDurationLeakCount", "raw duration leak数"), count("lastInputTimestampLeakCount", "last-input timestamp leak数")]),
];

export const AGENT_RUN_CASES: CaseSpec[] = [
  { caseId: "normal", buildClass: "signed-packaged", instruction: "normal read-only turnがstreamしterminal 1件、SQLite completedになることを確認する。", metrics: [{ key: "terminalEventCount", unit: "count", description: "terminal event数", exact: 1 }, { key: "completedRowCount", unit: "count", description: "completed row数", exact: 1 }] },
  { caseId: "long-reasoning", buildClass: "signed-packaged", instruction: "meaningful progressを伴うlong reasoningがtimeoutせず完了することを確認する。", metrics: [atLeast("meaningfulProgressCount", "meaningful progress数", 2), count("falseProgressTimeoutCount", "誤progress timeout数"), { key: "terminalEventCount", unit: "count", description: "terminal event数", exact: 1 }] },
  { caseId: "cancel-single", buildClass: "signed-packaged", instruction: "単一Cancelでinterrupt最大1、child/task残存0を確認する。", metrics: [{ key: "interruptCount", unit: "count", description: "interrupt送信数", max: 1 }, count("childProcessRemainingCount", "残存child process数"), count("asyncTaskRemainingCount", "残存async task数")] },
  { caseId: "cancel-repeated", buildClass: "signed-packaged", instruction: "Cancelを10回連打しinterrupt最大1、child/task残存0を確認する。", metrics: [{ key: "cancelRequestCount", unit: "count", description: "Cancel操作回数", exact: 10 }, { key: "interruptCount", unit: "count", description: "interrupt送信数", max: 1 }, count("childProcessRemainingCount", "残存child process数"), count("asyncTaskRemainingCount", "残存async task数")] },
  { caseId: "window-close", buildClass: "signed-packaged", instruction: "active run中の通常window closeでappとCodex childが5秒以内に終了することを確認する。", metrics: [latency("appProcessExitMs", "app process終了時間"), latency("childProcessExitMs", "Codex child終了時間"), count("runningRowRemainingCount", "残存running row数")] },
  { caseId: "forced-restart", buildClass: "signed-packaged", instruction: "強制終了後のrestartでrunning rowがinterrupted/app-restartedになることを確認する。", metrics: [count("runningRowRemainingCount", "残存running row数"), atLeast("interruptedRowCount", "interrupted row数"), atLeast("appRestartedReasonCount", "app-restarted reason数")] },
  { caseId: "read-only-violation", buildClass: "signed-packaged", instruction: "workspace変更要求が拒否され、approval/Network/Web Search/write MCPが0件であることを確認する。", metrics: [count("workspaceChangeCount", "workspace変更数"), count("approvalUiCount", "approval UI表示数"), count("networkRequestCount", "Network request数"), count("webSearchCount", "Web Search数"), count("writeMcpCount", "write-capable MCP数")] },
].map((spec) => ({ ...spec, metrics: [...spec.metrics, { ...count("workspaceDigestMismatchCount", "fixture workspace digest不一致数"), automatic: "workspace-integrity" }, count("rawProviderLeakCount", "raw provider data leak数")] }));

export function soakCase(mode: "soak-30m" | "soak-2h"): CaseSpec {
  const twoHours = mode === "soak-2h";
  return {
    caseId: mode,
    buildClass: "signed-packaged",
    instruction: `${twoHours ? "2時間" : "30分"}のreal ASR soakを開始し、指示されたpause/resume/stopを実施する。`,
    metrics: [
      { key: "elapsedSeconds", unit: "seconds", description: "runner計測時間", min: twoHours ? 7_200 : 1_800, automatic: "elapsed-seconds" },
      { key: "rssMedianDeltaMiB", unit: "mib", description: "先頭・末尾windowのRSS中央値差", max: twoHours ? 64 : 48, automatic: "rss-median-delta" },
      { key: "maxQueueDepth", unit: "count", description: "最大segment queue深度", max: 2 },
      { key: "maxInFlightAsr", unit: "count", description: "最大in-flight ASR数", max: 1 },
      count("childProcessRemainingCount", "終了後の残存child process数"),
      count("unexpectedTranscriptPersistenceCount", "予期しないtranscript永続化数"),
      latency("indicatorReleaseMs", "停止後のmicrophone indicator消灯時間"),
    ],
  };
}

export function caseSpecs(suite: Suite, mode: Mode): CaseSpec[] {
  if (suite === "meeting" && mode === "functional") return [...MEETING_FUNCTIONAL_CASES].sort((left, right) => BUILD_CLASSES.indexOf(left.buildClass) - BUILD_CLASSES.indexOf(right.buildClass));
  if (suite === "meeting" && (mode === "soak-30m" || mode === "soak-2h")) return [soakCase(mode)];
  if (suite === "input-activity" && mode === "manual") return [...INPUT_ACTIVITY_CASES].sort((left, right) => BUILD_CLASSES.indexOf(left.buildClass) - BUILD_CLASSES.indexOf(right.buildClass));
  if (suite === "agent-run" && mode === "manual") return AGENT_RUN_CASES;
  throw new RunnerError(64, "usage-error");
}

export const utcTimestamp = z.string().datetime({ offset: false });
export const sha256 = z.string().regex(/^[0-9a-f]{64}$/);
export const commit = z.string().regex(/^[0-9a-f]{7,64}$/);
export const safeToken = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);

export const identitySchema = z.object({
  saaaCommit: commit,
  bundleSha256: sha256,
  osVersion: z.string().regex(/^[A-Za-z0-9._() -]{1,96}$/),
  architecture: z.literal("arm64"),
  signingClass: z.enum(["developer-id-application", "apple-development"]),
  operator: safeToken,
}).strict();

export const observationSchema = z.object({
  key: z.string().regex(/^[A-Za-z][A-Za-z0-9]{0,63}$/),
  value: z.number().finite().min(0).max(10_000_000),
  unit: z.enum(["count", "milliseconds", "seconds", "mib"]),
}).strict();

export const caseResultSchema = z.object({
  caseId: z.string().regex(/^[a-z0-9-]{1,64}$/),
  buildClass: z.enum(BUILD_CLASSES),
  startedAt: utcTimestamp,
  completedAt: utcTimestamp,
  result: z.enum(RESULT_VALUES),
  reasonCode: z.enum(REASON_CODES).nullable(),
  observations: z.array(observationSchema).max(24),
}).strict().superRefine((value, context) => {
  if (Date.parse(value.completedAt) < Date.parse(value.startedAt)) context.addIssue({ code: "custom", path: ["completedAt"], message: "completedAt precedes startedAt" });
  if ((value.result === "pass") !== (value.reasonCode === null)) context.addIssue({ code: "custom", path: ["reasonCode"], message: "pass requires null reason and non-pass requires a reason" });
  if (value.result === "pass" && value.observations.length === 0) context.addIssue({ code: "custom", path: ["observations"], message: "pass requires bounded observations" });
});

export const preflightReportSchema = z.object({
  schemaVersion: z.literal(SCHEMA_VERSION),
  suite: z.literal("preflight"),
  mode: z.literal("preflight"),
  identity: identitySchema,
  startedAt: utcTimestamp,
  completedAt: utcTimestamp,
  workspaceInitialSha256: sha256,
  dedicatedAppDataEmpty: z.literal(true),
  result: z.literal("pass"),
}).strict();

export const suiteReportSchema = z.object({
  schemaVersion: z.literal(SCHEMA_VERSION),
  suite: z.enum(SUITES),
  mode: z.enum(MODES),
  identity: identitySchema,
  startedAt: utcTimestamp,
  completedAt: utcTimestamp,
  cases: z.array(caseResultSchema).min(1).max(64),
  result: z.enum(RESULT_VALUES),
}).strict().superRefine((report, context) => {
  if (Date.parse(report.completedAt) < Date.parse(report.startedAt)) context.addIssue({ code: "custom", path: ["completedAt"], message: "completedAt precedes startedAt" });
  const keys = report.cases.map((item) => `${item.caseId}:${item.buildClass}`);
  if (new Set(keys).size !== keys.length) context.addIssue({ code: "custom", path: ["cases"], message: "duplicate case result" });
  const derived = report.cases.some((item) => item.result === "fail") ? "fail" : report.cases.some((item) => item.result === "blocked") ? "blocked" : "pass";
  if (report.result !== derived) context.addIssue({ code: "custom", path: ["result"], message: "suite result disagrees with cases" });
});

export const aggregateReportSchema = z.object({
  schemaVersion: z.literal(SCHEMA_VERSION),
  suite: z.literal("aggregate"),
  mode: z.literal("aggregate"),
  identity: identitySchema,
  startedAt: utcTimestamp,
  completedAt: utcTimestamp,
  expectedCaseCount: z.number().int().min(1).max(256),
  passedCaseCount: z.number().int().min(0).max(256),
  failedCaseCount: z.number().int().min(0).max(256),
  blockedCaseCount: z.number().int().min(0).max(256),
  missingCaseCount: z.number().int().min(0).max(256),
  forbiddenDataFindingCount: z.number().int().min(0).max(256),
  reportSetSha256: sha256,
  result: z.enum(["accepted", "not-accepted"]),
}).strict().superRefine((report, context) => {
  const accepted = report.passedCaseCount === report.expectedCaseCount && report.failedCaseCount === 0 && report.blockedCaseCount === 0 && report.missingCaseCount === 0 && report.forbiddenDataFindingCount === 0;
  if ((report.result === "accepted") !== accepted) context.addIssue({ code: "custom", path: ["result"], message: "aggregate result disagrees with counts" });
});

export type Identity = z.infer<typeof identitySchema>;
export type PreflightReport = z.infer<typeof preflightReportSchema>;
export type SuiteReport = z.infer<typeof suiteReportSchema>;
export type AggregateReport = z.infer<typeof aggregateReportSchema>;

export interface CliArguments {
  command: "preflight" | "verify" | "report";
  reportDirectory: string;
  suite?: Suite;
  mode?: Mode;
}

export class RunnerError extends Error {
  constructor(readonly exitCode: 2 | 3 | 64 | 70, readonly code: string) {
    super(code);
  }
}
