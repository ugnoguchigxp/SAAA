import { createHash, randomBytes } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import {
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  openSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  unlinkSync,
  writeSync,
  linkSync,
  mkdtempSync,
  rmSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { createInterface } from "node:readline/promises";
import { fileURLToPath } from "node:url";
import { z } from "zod";

export const SCHEMA_VERSION = "saaa-mvp-2-2.5-readiness-v1" as const;
const ROOT = fileURLToPath(new URL("..", import.meta.url));
const DEFAULT_BUNDLE = join(ROOT, "src-tauri/target/release/bundle/macos/SAAA.app");
const DEFAULT_DEVELOPMENT_EXECUTABLE = join(ROOT, "src-tauri/target/debug/bundle/macos/SAAA.app/Contents/MacOS/saaa");
const BUNDLE_IDENTIFIER = "com.saaa.desktop";
const MAX_REPORT_BYTES = 256 * 1024;
const APPLE_ROOT_SHA256 = new Set([
  "B0B1730ECBC7FF4505142C49F1295E6EDA6BCAED7E2C68C5BE91B5A11001F024",
  "C2B9B042DD57830E7D117DAC55AC8AE19407D38E41D88F3215BC3A890444A050",
  "63343ABFB89A6A03EBB57E9B3F5FA7BE7C4F5C756F3017B3A8C488C3653E9179",
]);

const REASON_CODES = [
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

const RESULT_VALUES = ["pass", "fail", "blocked"] as const;
const BUILD_CLASSES = ["development", "signed-packaged"] as const;
const SUITES = ["meeting", "input-activity", "agent-run"] as const;
const MODES = ["functional", "soak-30m", "soak-2h", "manual"] as const;

type Suite = typeof SUITES[number];
type Mode = typeof MODES[number];
type BuildClass = typeof BUILD_CLASSES[number];
type Result = typeof RESULT_VALUES[number];
type ReasonCode = typeof REASON_CODES[number];

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

const count = (key: string, description: string, exact = 0): MetricSpec => ({ key, unit: "count", description, exact });
const atLeast = (key: string, description: string, min = 1): MetricSpec => ({ key, unit: "count", description, min });
const latency = (key: string, description: string, max = 5_000): MetricSpec => ({ key, unit: "milliseconds", description, max });

function forBuilds(caseId: string, instruction: string, metrics: MetricSpec[]): CaseSpec[] {
  return BUILD_CLASSES.map((buildClass) => ({ caseId, buildClass, instruction, metrics }));
}

const MEETING_FUNCTIONAL_CASES: CaseSpec[] = [
  ...forBuilds("permission-grant", "未決定状態からmicrophoneを許可し、capture開始とtrack解放を確認する。", [atLeast("captureStartCount", "capture開始回数"), latency("trackReleaseMs", "track解放時間")]),
  ...forBuilds("permission-deny", "microphoneを拒否し、capture 0とrecovery表示を確認する。", [count("captureStartCount", "capture開始回数"), atLeast("recoveryVisibleCount", "recovery表示回数")]),
  ...forBuilds("permission-loss", "active中にpermission revokeまたはdevice lossを発生させ、安全遷移とcleanupを確認する。", [count("captureResourceRemainingCount", "残存capture resource数"), count("asrTaskRemainingCount", "残存ASR task数"), latency("indicatorReleaseMs", "microphone indicator消灯時間")]),
  ...forBuilds("partial-final", "real gnosis ASRで同一lane/sequenceのPartialがFinalへ置換されることを確認する。", [atLeast("partialFinalReplacementCount", "Partial→Final置換回数"), count("sequenceViolationCount", "sequence重複・逆行回数")]),
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

const INPUT_ACTIVITY_CASES: CaseSpec[] = [
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

const AGENT_RUN_CASES: CaseSpec[] = [
  { caseId: "normal", buildClass: "signed-packaged", instruction: "normal read-only turnがstreamしterminal 1件、SQLite completedになることを確認する。", metrics: [{ key: "terminalEventCount", unit: "count", description: "terminal event数", exact: 1 }, { key: "completedRowCount", unit: "count", description: "completed row数", exact: 1 }] },
  { caseId: "long-reasoning", buildClass: "signed-packaged", instruction: "meaningful progressを伴うlong reasoningがtimeoutせず完了することを確認する。", metrics: [atLeast("meaningfulProgressCount", "meaningful progress数", 2), count("falseProgressTimeoutCount", "誤progress timeout数"), { key: "terminalEventCount", unit: "count", description: "terminal event数", exact: 1 }] },
  { caseId: "cancel-single", buildClass: "signed-packaged", instruction: "単一Cancelでinterrupt最大1、child/task残存0を確認する。", metrics: [{ key: "interruptCount", unit: "count", description: "interrupt送信数", max: 1 }, count("childProcessRemainingCount", "残存child process数"), count("asyncTaskRemainingCount", "残存async task数")] },
  { caseId: "cancel-repeated", buildClass: "signed-packaged", instruction: "Cancelを10回連打しinterrupt最大1、child/task残存0を確認する。", metrics: [{ key: "cancelRequestCount", unit: "count", description: "Cancel操作回数", exact: 10 }, { key: "interruptCount", unit: "count", description: "interrupt送信数", max: 1 }, count("childProcessRemainingCount", "残存child process数"), count("asyncTaskRemainingCount", "残存async task数")] },
  { caseId: "window-close", buildClass: "signed-packaged", instruction: "active run中の通常window closeでappとCodex childが5秒以内に終了することを確認する。", metrics: [latency("appProcessExitMs", "app process終了時間"), latency("childProcessExitMs", "Codex child終了時間"), count("runningRowRemainingCount", "残存running row数")] },
  { caseId: "forced-restart", buildClass: "signed-packaged", instruction: "強制終了後のrestartでrunning rowがinterrupted/app-restartedになることを確認する。", metrics: [count("runningRowRemainingCount", "残存running row数"), atLeast("interruptedRowCount", "interrupted row数"), atLeast("appRestartedReasonCount", "app-restarted reason数")] },
  { caseId: "read-only-violation", buildClass: "signed-packaged", instruction: "workspace変更要求が拒否され、approval/Network/Web Search/write MCPが0件であることを確認する。", metrics: [count("workspaceChangeCount", "workspace変更数"), count("approvalUiCount", "approval UI表示数"), count("networkRequestCount", "Network request数"), count("webSearchCount", "Web Search数"), count("writeMcpCount", "write-capable MCP数")] },
].map((spec) => ({ ...spec, metrics: [...spec.metrics, { ...count("workspaceDigestMismatchCount", "fixture workspace digest不一致数"), automatic: "workspace-integrity" }, count("rawProviderLeakCount", "raw provider data leak数")] }));

function soakCase(mode: "soak-30m" | "soak-2h"): CaseSpec {
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

const utcTimestamp = z.string().datetime({ offset: false });
const sha256 = z.string().regex(/^[0-9a-f]{64}$/);
const commit = z.string().regex(/^[0-9a-f]{7,64}$/);
const safeToken = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);

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

export function parseCliArguments(argv: string[]): CliArguments {
  const command = argv[0];
  if (command !== "preflight" && command !== "verify" && command !== "report") throw new RunnerError(64, "usage-error");
  let reportDirectory: string | undefined;
  let suite: Suite | undefined;
  let mode: Mode | undefined;
  for (let index = 1; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (flag === "--report-dir" && reportDirectory === undefined && value && !value.startsWith("--")) reportDirectory = value;
    else if (flag === "--suite" && suite === undefined && SUITES.includes(value as Suite)) suite = value as Suite;
    else if (flag === "--mode" && mode === undefined && MODES.includes(value as Mode)) mode = value as Mode;
    else throw new RunnerError(64, "usage-error");
    index += 1;
  }
  if (!reportDirectory || !isAbsolute(reportDirectory)) throw new RunnerError(64, "usage-error");
  if ((command === "verify") !== Boolean(suite && mode)) throw new RunnerError(64, "usage-error");
  if (command === "verify") caseSpecs(suite!, mode!);
  return { command, reportDirectory, ...(suite ? { suite } : {}), ...(mode ? { mode } : {}) };
}

function commandOutput(command: string, args: string[], cwd = ROOT): string {
  try {
    return execFileSync(command, args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  } catch {
    throw new RunnerError(3, "environment-invalid");
  }
}

function requireDirectory(path: string, empty: boolean): string {
  if (!isAbsolute(path) || !existsSync(path)) throw new RunnerError(3, "environment-invalid");
  const info = lstatSync(path);
  if (!info.isDirectory() || info.isSymbolicLink()) throw new RunnerError(3, "environment-invalid");
  const canonical = realpathSync(path);
  if ((info.mode & 0o777) !== 0o700) throw new RunnerError(3, "environment-invalid");
  if (empty && readdirSync(canonical).length !== 0) throw new RunnerError(3, "report-directory-not-empty");
  return canonical;
}

function isWithin(parent: string, candidate: string): boolean {
  const path = relative(parent, candidate);
  return path === "" || (!path.startsWith(`..${sep}`) && path !== ".." && !isAbsolute(path));
}

export function directoriesOverlap(left: string, right: string): boolean {
  return isWithin(left, right) || isWithin(right, left);
}

export function hashDirectory(root: string, excludeGit = false): string {
  const hash = createHash("sha256");
  const walk = (directory: string) => {
    for (const name of readdirSync(directory).sort()) {
      if (excludeGit && directory === root && name === ".git") continue;
      const path = join(directory, name);
      const info = lstatSync(path);
      const key = relative(root, path).split(sep).join("/");
      if (info.isDirectory()) {
        hash.update(`d\0${key}\0${info.mode & 0o777}\0`);
        walk(path);
      } else if (info.isFile()) {
        if (info.nlink !== 1) throw new RunnerError(3, "environment-invalid");
        hash.update(`f\0${key}\0${info.mode & 0o777}\0${info.size}\0`);
        hash.update(readFileSync(path));
      } else if (info.isSymbolicLink()) {
        hash.update(`l\0${key}\0${readlinkSync(path)}\0`);
      } else {
        throw new RunnerError(3, "environment-invalid");
      }
    }
  };
  walk(root);
  return hash.digest("hex");
}

export function classifySigningDetails(details: string, certificateText: string, rootFingerprint: string): Identity["signingClass"] {
  if (/Signature=adhoc/i.test(details)) throw new RunnerError(3, "signature-invalid");
  if (!APPLE_ROOT_SHA256.has(rootFingerprint.replaceAll(":", "").toUpperCase())) throw new RunnerError(3, "signing-class-invalid");
  if (!/^TeamIdentifier=[A-Z0-9]{10}$/m.test(details)) throw new RunnerError(3, "signing-class-invalid");
  if (
    /^Authority=Developer ID Application: .+$/m.test(details)
    && /^Authority=Developer ID Certification Authority$/m.test(details)
    && /1\.2\.840\.113635\.100\.6\.1\.13/.test(certificateText)
  ) return "developer-id-application";
  if (
    /^Authority=Apple Development: .+$/m.test(details)
    && /^Authority=Apple Worldwide Developer Relations Certification Authority$/m.test(details)
    && /1\.2\.840\.113635\.100\.6\.1\.12/.test(certificateText)
  ) return "apple-development";
  throw new RunnerError(3, "signing-class-invalid");
}

function signingClass(bundlePath: string): Identity["signingClass"] {
  const verified = spawnSync("codesign", ["--verify", "--deep", "--strict", bundlePath], { encoding: "utf8" });
  if (verified.status !== 0) throw new RunnerError(3, "signature-invalid");
  const certificateDirectory = mkdtempSync(join(tmpdir(), "saaa-mvp2x-certificate-"));
  try {
    const details = spawnSync(
      "codesign",
      ["--display", "--verbose=4", "--extract-certificates=certificate", bundlePath],
      { cwd: certificateDirectory, encoding: "utf8" },
    );
    const output = `${details.stdout ?? ""}\n${details.stderr ?? ""}`;
    const certificates = readdirSync(certificateDirectory)
      .filter((name) => /^certificate\d+$/.test(name))
      .sort((left, right) => Number(left.slice("certificate".length)) - Number(right.slice("certificate".length)))
      .map((name) => join(certificateDirectory, name));
    if (details.status !== 0 || certificates.length < 3) throw new RunnerError(3, "signature-invalid");
    const certificate = certificates[0]!;
    const rootCertificate = certificates.at(-1)!;
    const verificationArguments = [
      "verify-cert",
      ...certificates.slice(0, -1).flatMap((path) => ["-c", path]),
      "-r",
      rootCertificate,
      "-p",
      "codeSign",
      "-N",
      "-L",
    ];
    const trusted = spawnSync("security", verificationArguments, { encoding: "utf8" });
    if (trusted.status !== 0) throw new RunnerError(3, "signature-invalid");
    const decoded = spawnSync("openssl", ["x509", "-in", certificate, "-inform", "DER", "-noout", "-text"], { encoding: "utf8" });
    if (decoded.status !== 0) throw new RunnerError(3, "signature-invalid");
    const root = spawnSync("openssl", ["x509", "-in", rootCertificate, "-inform", "DER", "-noout", "-fingerprint", "-sha256"], { encoding: "utf8" });
    const rootFingerprint = root.stdout?.match(/sha256 Fingerprint=([0-9A-F:]+)/i)?.[1];
    if (root.status !== 0 || !rootFingerprint) throw new RunnerError(3, "signature-invalid");
    return classifySigningDetails(output, decoded.stdout ?? "", rootFingerprint);
  } finally {
    rmSync(certificateDirectory, { recursive: true, force: true });
  }
}

function bundleIdentity(bundlePath: string): Omit<Identity, "operator"> {
  const executable = join(bundlePath, "Contents/MacOS/saaa");
  if (!existsSync(executable)) throw new RunnerError(3, "bundle-missing");
  const identifier = commandOutput("/usr/libexec/PlistBuddy", ["-c", "Print:CFBundleIdentifier", join(bundlePath, "Contents/Info.plist")]);
  if (identifier !== BUNDLE_IDENTIFIER) throw new RunnerError(3, "bundle-identifier-invalid");
  const architecture = commandOutput("uname", ["-m"]);
  if (architecture !== "arm64" || !commandOutput("file", [executable]).includes("arm64")) throw new RunnerError(3, "architecture-invalid");
  const productVersion = commandOutput("sw_vers", ["-productVersion"]);
  const buildVersion = commandOutput("sw_vers", ["-buildVersion"]);
  return {
    saaaCommit: commandOutput("git", ["rev-parse", "HEAD"]),
    bundleSha256: hashDirectory(bundlePath),
    osVersion: `macOS ${productVersion} (${buildVersion})`,
    architecture: "arm64",
    signingClass: signingClass(bundlePath),
  };
}

function requiredEnvironmentDirectory(name: string, empty: boolean): string {
  const value = process.env[name];
  if (!value) throw new RunnerError(3, "environment-variable-missing");
  return requireDirectory(value, empty);
}

function configuredBundlePath(): string {
  const bundlePath = process.env.SAAA_MVP2X_BUNDLE_PATH ?? DEFAULT_BUNDLE;
  if (!isAbsolute(bundlePath) || !existsSync(bundlePath) || lstatSync(bundlePath).isSymbolicLink()) throw new RunnerError(3, "bundle-missing");
  return realpathSync(bundlePath);
}

function configuredOperator(): string {
  const operator = process.env.SAAA_MVP2X_OPERATOR;
  if (!operator || !safeToken.safeParse(operator).success) throw new RunnerError(3, "operator-invalid");
  return operator;
}

function isolatedEnvironmentDirectories(reportDirectory: string, appDataEmpty: boolean): { appDataDirectory: string; workspaceDirectory: string } {
  const appDataDirectory = requiredEnvironmentDirectory("SAAA_MVP2X_APP_DATA_DIR", appDataEmpty);
  const workspaceDirectory = requiredEnvironmentDirectory("SAAA_MVP2X_WORKSPACE_DIR", false);
  if (
    directoriesOverlap(ROOT, appDataDirectory)
    || directoriesOverlap(ROOT, workspaceDirectory)
    || directoriesOverlap(reportDirectory, appDataDirectory)
    || directoriesOverlap(reportDirectory, workspaceDirectory)
    || directoriesOverlap(appDataDirectory, workspaceDirectory)
  ) throw new RunnerError(3, "environment-not-isolated");
  const normalAppData = join(homedir(), "Library/Application Support", BUNDLE_IDENTIFIER);
  if (resolve(appDataDirectory) === resolve(normalAppData)) throw new RunnerError(3, "normal-app-data-refused");
  if (!existsSync(join(workspaceDirectory, ".git")) || commandOutput("git", ["status", "--porcelain", "--untracked-files=all"], workspaceDirectory) !== "") throw new RunnerError(3, "fixture-workspace-invalid");
  return { appDataDirectory, workspaceDirectory };
}

function currentIdentity(): Identity {
  return { ...bundleIdentity(configuredBundlePath()), operator: configuredOperator() };
}

function assertCurrentIdentity(expected: Identity): void {
  if (JSON.stringify(currentIdentity()) !== JSON.stringify(expected)) throw new RunnerError(3, "identity-mismatch");
}

function assertVerificationEnvironment(reportDirectory: string, preflight: PreflightReport): string {
  if (isWithin(ROOT, reportDirectory)) throw new RunnerError(3, "report-directory-inside-repository");
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "") throw new RunnerError(3, "dirty-tree");
  const { appDataDirectory, workspaceDirectory } = isolatedEnvironmentDirectories(reportDirectory, false);
  const databasePath = join(appDataDirectory, "saaa.sqlite3");
  if (!existsSync(databasePath)) throw new RunnerError(3, "dedicated-app-data-unused");
  const database = lstatSync(databasePath);
  if (!database.isFile() || database.isSymbolicLink() || database.nlink !== 1) throw new RunnerError(3, "dedicated-app-data-unused");
  const schemaTableCount = commandOutput("/usr/bin/sqlite3", [
    databasePath,
    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('settings_documents','conversations','runtime_runs','meeting_sessions');",
  ]);
  if (schemaTableCount !== "4") throw new RunnerError(3, "dedicated-app-data-unused");
  if (hashDirectory(workspaceDirectory, true) !== preflight.workspaceInitialSha256) throw new RunnerError(3, "fixture-workspace-invalid");
  assertCurrentIdentity(preflight.identity);
  return workspaceDirectory;
}

export function assertMetric(metric: MetricSpec, value: number): void {
  if (!Number.isFinite(value) || value < 0 || value > 10_000_000) throw new RunnerError(3, "observation-invalid");
  if (metric.exact !== undefined && value !== metric.exact) throw new RunnerError(2, "threshold-exceeded");
  if (metric.min !== undefined && value < metric.min) throw new RunnerError(2, "threshold-exceeded");
  if (metric.max !== undefined && value > metric.max) throw new RunnerError(2, "threshold-exceeded");
}

const FORBIDDEN_KEYS = new Set([
  "credential", "token", "authorization", "endpoint", "host", "privateIp", "prompt", "response",
  "transcriptText", "audio", "workspacePath", "databasePath", "homeDirectory", "threadId", "turnId",
  "requestId", "windowTitle", "applicationIdentifier", "rawInputDuration",
]);
const FORBIDDEN_TEXT = /(authorization\s*:|bearer\s+|https?:\/\/|\/Users\/|\/home\/|[A-Za-z]:\\|\b(?:\d{1,3}\.){3}\d{1,3}\b|\bssh\s+|\b[A-Za-z0-9._-]+@[A-Za-z0-9._-]+\b)/i;

export function forbiddenDataFindings(value: unknown): number {
  let findings = 0;
  const visit = (item: unknown) => {
    if (typeof item === "string") {
      if (FORBIDDEN_TEXT.test(item)) findings += 1;
      return;
    }
    if (Array.isArray(item)) {
      for (const entry of item) visit(entry);
      return;
    }
    if (item && typeof item === "object") {
      for (const [key, entry] of Object.entries(item)) {
        if (FORBIDDEN_KEYS.has(key)) findings += 1;
        visit(entry);
      }
    }
  };
  visit(value);
  return findings;
}

export function writeJsonExclusive(reportDirectory: string, filename: string, value: unknown): void {
  if (!/^[a-z0-9.-]+\.json$/.test(filename) || forbiddenDataFindings(value) !== 0) throw new RunnerError(70, "redaction-failed");
  const encoded = `${JSON.stringify(value, null, 2)}\n`;
  if (Buffer.byteLength(encoded) > MAX_REPORT_BYTES) throw new RunnerError(70, "report-too-large");
  const temporary = join(reportDirectory, `.${filename}.${randomBytes(12).toString("hex")}.tmp`);
  const target = join(reportDirectory, filename);
  const descriptor = openSync(temporary, "wx", 0o600);
  try {
    try {
      const bytes = Buffer.from(encoded, "utf8");
      let offset = 0;
      while (offset < bytes.length) {
        const written = writeSync(descriptor, bytes, offset, bytes.length - offset);
        if (written <= 0) throw new RunnerError(70, "atomic-write-failed");
        offset += written;
      }
      fsyncSync(descriptor);
    } finally {
      closeSync(descriptor);
    }
  } catch {
    if (existsSync(temporary)) unlinkSync(temporary);
    throw new RunnerError(70, "atomic-write-failed");
  }
  let targetLinked = false;
  try {
    linkSync(temporary, target);
    targetLinked = true;
    const directoryDescriptor = openSync(reportDirectory, "r");
    try { fsyncSync(directoryDescriptor); } finally { closeSync(directoryDescriptor); }
  } catch (cause) {
    if (targetLinked && existsSync(target)) unlinkSync(target);
    if (existsSync(temporary)) unlinkSync(temporary);
    if ((cause as NodeJS.ErrnoException).code === "EEXIST") throw new RunnerError(3, "report-overwrite-refused");
    throw new RunnerError(70, "atomic-write-failed");
  }
  unlinkSync(temporary);
}

function readJson(path: string): unknown {
  const info = lstatSync(path);
  if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || info.size > MAX_REPORT_BYTES || (info.mode & 0o777) !== 0o600) throw new RunnerError(3, "report-file-invalid");
  try { return JSON.parse(readFileSync(path, "utf8")); } catch { throw new RunnerError(3, "report-schema-invalid"); }
}

function reportFilename(suite: Suite, mode: Mode): string {
  return `${suite}-${mode}.json`;
}

function sameIdentity(left: Identity, right: Identity): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function runPreflight(reportDirectoryInput: string): PreflightReport {
  const startedAt = new Date().toISOString();
  const reportDirectory = requireDirectory(reportDirectoryInput, true);
  if (isWithin(ROOT, reportDirectory)) throw new RunnerError(3, "report-directory-inside-repository");
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "") throw new RunnerError(3, "dirty-tree");
  const { appDataDirectory, workspaceDirectory } = isolatedEnvironmentDirectories(reportDirectory, true);
  const report = preflightReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite: "preflight",
    mode: "preflight",
    identity: currentIdentity(),
    startedAt,
    completedAt: new Date().toISOString(),
    workspaceInitialSha256: hashDirectory(workspaceDirectory, true),
    dedicatedAppDataEmpty: readdirSync(appDataDirectory).length === 0,
    result: "pass",
  });
  writeJsonExclusive(reportDirectory, "preflight.json", report);
  return report;
}

export interface RssSample { atMs: number; rssMiB: number }

export function median(values: number[]): number {
  if (values.length === 0) throw new RunnerError(3, "sampling-gap");
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[middle - 1]! + sorted[middle]!) / 2 : sorted[middle]!;
}

export function summarizeRssMedianDelta(samples: RssSample[], durationMs: number): number {
  const start = samples[0]?.atMs;
  if (start === undefined) throw new RunnerError(3, "sampling-gap");
  const twoHours = durationMs >= 7_200_000;
  const warmupMs = twoHours ? 300_000 : 60_000;
  const windowMs = twoHours ? 900_000 : 300_000;
  const first = samples.filter((sample) => sample.atMs >= start + warmupMs && sample.atMs < start + warmupMs + windowMs).map((sample) => sample.rssMiB);
  const end = start + durationMs;
  const last = samples.filter((sample) => sample.atMs > end - windowMs && sample.atMs <= end).map((sample) => sample.rssMiB);
  return Math.max(0, median(last) - median(first));
}

function sampleRss(pid: number): number {
  const command = commandOutput("ps", ["-p", String(pid), "-o", "rss="]);
  const kib = Number(command);
  if (!Number.isInteger(kib) || kib <= 0) throw new RunnerError(3, "process-exited");
  return kib / 1024;
}

function expectedAppExecutable(buildClass: BuildClass): string {
  return buildClass === "signed-packaged"
    ? join(configuredBundlePath(), "Contents/MacOS/saaa")
    : DEFAULT_DEVELOPMENT_EXECUTABLE;
}

function validatedAppPid(buildClass: BuildClass, suppliedPid?: number): number {
  const pid = suppliedPid ?? Number(process.env.SAAA_MVP2X_APP_PID);
  if (!Number.isInteger(pid) || pid <= 1) throw new RunnerError(3, "app-pid-invalid");
  const command = commandOutput("ps", ["-p", String(pid), "-o", "command="]);
  const executable = expectedAppExecutable(buildClass);
  if (!existsSync(executable) || lstatSync(executable).isSymbolicLink()) throw new RunnerError(3, "app-pid-invalid");
  if (command !== executable && !command.startsWith(`${executable} `)) throw new RunnerError(3, "app-pid-invalid");
  return pid;
}

async function promptAndValidateAppPid(
  reader: ReturnType<typeof createInterface>,
  spec: CaseSpec,
): Promise<void> {
  process.stdout.write(`\n[${spec.buildClass}] ${spec.caseId}\n${spec.instruction}\n`);
  const suppliedPid = Number((await reader.question("実行中の対象app PID: ")).trim());
  validatedAppPid(spec.buildClass, suppliedPid);
}

function workspaceIntegrityMismatch(workspaceDirectory: string, expectedSha256: string): number {
  try {
    const dirty = commandOutput("git", ["status", "--porcelain", "--untracked-files=all"], workspaceDirectory) !== "";
    return dirty || hashDirectory(workspaceDirectory, true) !== expectedSha256 ? 1 : 0;
  } catch {
    return 1;
  }
}

async function collectRss(pid: number, durationMs: number): Promise<RssSample[]> {
  const samples: RssSample[] = [];
  const started = Date.now();
  let nextProgress = 300_000;
  while (true) {
    const elapsed = Date.now() - started;
    samples.push({ atMs: started + elapsed, rssMiB: sampleRss(pid) });
    if (elapsed >= durationMs) break;
    if (elapsed >= nextProgress) {
      process.stdout.write(`resource sampling: ${Math.floor(elapsed / 60_000)} minutes complete\n`);
      nextProgress += 300_000;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, Math.min(5_000, durationMs - elapsed)));
  }
  return samples;
}

async function promptCase(
  reader: ReturnType<typeof createInterface>,
  spec: CaseSpec,
  automatic: Map<string, number> | (() => Map<string, number>),
  startedAtOverride?: string,
  skipReady = false,
  introDisplayed = false,
): Promise<z.infer<typeof caseResultSchema>> {
  if (!skipReady) {
    if (!introDisplayed) process.stdout.write(`\n[${spec.buildClass}] ${spec.caseId}\n${spec.instruction}\n`);
    await reader.question("準備できたらEnter: ");
  }
  const startedAt = startedAtOverride ?? new Date().toISOString();
  const resultInput = (await reader.question("result (pass/fail/blocked): ")).trim();
  if (!RESULT_VALUES.includes(resultInput as Result)) throw new RunnerError(3, "observation-invalid");
  let result = resultInput as Result;
  let reasonCode: ReasonCode | null = null;
  const observations: Array<z.infer<typeof observationSchema>> = [];
  if (result === "pass") {
    const automaticObservations = typeof automatic === "function" ? automatic() : automatic;
    for (const metric of spec.metrics) {
      const supplied = automaticObservations.get(metric.key);
      const raw = supplied === undefined ? (await reader.question(`${metric.description} [${metric.unit}]: `)).trim() : String(supplied);
      const value = Number(raw);
      try {
        assertMetric(metric, value);
        observations.push({ key: metric.key, value, unit: metric.unit });
      } catch (cause) {
        if (cause instanceof RunnerError && cause.code === "threshold-exceeded") {
          observations.push({ key: metric.key, value, unit: metric.unit });
          result = "fail";
          reasonCode = "threshold-exceeded";
          break;
        }
        throw cause;
      }
    }
  } else {
    const input = (await reader.question(`reason (${REASON_CODES.join("/")}): `)).trim();
    if (!REASON_CODES.includes(input as ReasonCode)) throw new RunnerError(3, "observation-invalid");
    reasonCode = input as ReasonCode;
  }
  return caseResultSchema.parse({ caseId: spec.caseId, buildClass: spec.buildClass, startedAt, completedAt: new Date().toISOString(), result, reasonCode, observations });
}

export async function runVerify(reportDirectoryInput: string, suite: Suite, mode: Mode): Promise<SuiteReport> {
  const reportDirectory = requireDirectory(reportDirectoryInput, false);
  const preflight = preflightReportSchema.parse(readJson(join(reportDirectory, "preflight.json")));
  const workspaceDirectory = assertVerificationEnvironment(reportDirectory, preflight);
  if (!process.stdin.isTTY || !process.stdout.isTTY) throw new RunnerError(3, "interactive-operator-required");
  const specs = caseSpecs(suite, mode);
  const startedAt = new Date().toISOString();
  const reader = createInterface({ input: process.stdin, output: process.stdout });
  const cases: SuiteReport["cases"] = [];
  try {
    for (let index = 0; index < specs.length; index += 1) {
      const spec = specs[index]!;
      const automatic = new Map<string, number>();
      let recorded: z.infer<typeof caseResultSchema>;
      const inputActivitySoak = suite === "input-activity" && spec.caseId === "sampling-soak";
      if (mode === "soak-30m" || mode === "soak-2h" || inputActivitySoak) {
        const pid = validatedAppPid(spec.buildClass);
        process.stdout.write(`\n${spec.instruction}\n`);
        await reader.question(`${inputActivitySoak ? "Input Activity monitoring" : "Meeting"}を開始してからEnterするとRSS samplingを開始します: `);
        const soakStartedAt = new Date().toISOString();
        const durationMs = mode === "soak-2h" ? 7_200_000 : 1_800_000;
        const samples = await collectRss(pid, durationMs);
        automatic.set("elapsedSeconds", durationMs / 1_000);
        automatic.set(inputActivitySoak ? "rssGrowthMiB" : "rssMedianDeltaMiB", summarizeRssMedianDelta(samples, durationMs));
        recorded = await promptCase(reader, spec, automatic, soakStartedAt, true);
      } else {
        if (suite === "agent-run" && workspaceIntegrityMismatch(workspaceDirectory, preflight.workspaceInitialSha256) !== 0) throw new RunnerError(3, "fixture-workspace-invalid");
        await promptAndValidateAppPid(reader, spec);
        recorded = await promptCase(
          reader,
          spec,
          suite === "agent-run"
            ? () => new Map([
              ["workspaceDigestMismatchCount", workspaceIntegrityMismatch(workspaceDirectory, preflight.workspaceInitialSha256)],
            ])
            : automatic,
          undefined,
          false,
          true,
        );
      }
      cases.push(recorded);
      if (recorded.result !== "pass") {
        const timestamp = new Date().toISOString();
        for (const skipped of specs.slice(index + 1)) {
          cases.push(caseResultSchema.parse({
            caseId: skipped.caseId,
            buildClass: skipped.buildClass,
            startedAt: timestamp,
            completedAt: timestamp,
            result: "blocked",
            reasonCode: "operator-blocked",
            observations: [],
          }));
        }
        break;
      }
    }
  } finally {
    reader.close();
  }
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "") throw new RunnerError(3, "dirty-tree");
  assertCurrentIdentity(preflight.identity);
  const result: Result = cases.some((item) => item.result === "fail") ? "fail" : cases.some((item) => item.result === "blocked") ? "blocked" : "pass";
  const report = suiteReportSchema.parse({ schemaVersion: SCHEMA_VERSION, suite, mode, identity: preflight.identity, startedAt, completedAt: new Date().toISOString(), cases, result });
  validateSuiteCases(report);
  writeJsonExclusive(reportDirectory, reportFilename(suite, mode), report);
  return report;
}

export function validateSuiteCases(report: SuiteReport): void {
  const expected = caseSpecs(report.suite, report.mode).map((item) => `${item.caseId}:${item.buildClass}`).sort();
  const actual = report.cases.map((item) => `${item.caseId}:${item.buildClass}`).sort();
  if (JSON.stringify(expected) !== JSON.stringify(actual)) throw new RunnerError(3, "case-matrix-invalid");
  for (const result of report.cases) {
    if (result.result !== "pass") continue;
    const spec = caseSpecs(report.suite, report.mode).find((item) => item.caseId === result.caseId && item.buildClass === result.buildClass)!;
    const metrics = new Map(result.observations.map((item) => [item.key, item]));
    if (metrics.size !== spec.metrics.length) throw new RunnerError(3, "observation-invalid");
    for (const metric of spec.metrics) {
      const observation = metrics.get(metric.key);
      if (!observation || observation.unit !== metric.unit) throw new RunnerError(3, "observation-invalid");
      assertMetric(metric, observation.value);
    }
  }
}

const EXPECTED_REPORTS: Array<[Suite, Mode]> = [
  ["meeting", "functional"],
  ["meeting", "soak-30m"],
  ["meeting", "soak-2h"],
  ["input-activity", "manual"],
  ["agent-run", "manual"],
];

export function hashEvidenceReportSet(reportDirectory: string): string {
  const hash = createHash("sha256");
  const filenames = [
    "preflight.json",
    ...EXPECTED_REPORTS.map(([suite, mode]) => reportFilename(suite, mode)),
  ];
  for (const filename of filenames) {
    const path = join(reportDirectory, filename);
    if (!existsSync(path)) continue;
    readJson(path);
    const bytes = readFileSync(path);
    hash.update(`${filename}\0${bytes.length}\0`);
    hash.update(bytes);
  }
  return hash.digest("hex");
}

function scanReportDirectory(reportDirectory: string): number {
  const allowed = new Set(["preflight.json", ...EXPECTED_REPORTS.map(([suite, mode]) => reportFilename(suite, mode))]);
  let findings = 0;
  for (const name of readdirSync(reportDirectory)) {
    const path = join(reportDirectory, name);
    const info = lstatSync(path);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || info.size > MAX_REPORT_BYTES || (info.mode & 0o777) !== 0o600) throw new RunnerError(3, "report-file-invalid");
    if (!allowed.has(name)) {
      findings += 1;
      if (FORBIDDEN_TEXT.test(readFileSync(path, "utf8"))) findings += 1;
      continue;
    }
    findings += forbiddenDataFindings(readJson(path));
  }
  return findings;
}

export function aggregateReports(reportDirectoryInput: string): AggregateReport {
  const startedAt = new Date().toISOString();
  const reportDirectory = requireDirectory(reportDirectoryInput, false);
  if (isWithin(ROOT, reportDirectory)) throw new RunnerError(3, "report-directory-inside-repository");
  const preflight = preflightReportSchema.parse(readJson(join(reportDirectory, "preflight.json")));
  const expectedCaseCount = EXPECTED_REPORTS.reduce((total, [suite, mode]) => total + caseSpecs(suite, mode).length, 0);
  let passedCaseCount = 0;
  let failedCaseCount = 0;
  let blockedCaseCount = 0;
  let missingCaseCount = 0;
  const forbiddenDataFindingCount = scanReportDirectory(reportDirectory);
  for (const [suite, mode] of EXPECTED_REPORTS) {
    const path = join(reportDirectory, reportFilename(suite, mode));
    if (!existsSync(path)) {
      missingCaseCount += caseSpecs(suite, mode).length;
      continue;
    }
    const raw = readJson(path);
    let report: SuiteReport;
    try {
      report = suiteReportSchema.parse(raw);
      if (report.suite !== suite || report.mode !== mode || !sameIdentity(report.identity, preflight.identity)) throw new RunnerError(3, "identity-mismatch");
      validateSuiteCases(report);
    } catch (cause) {
      if (cause instanceof RunnerError) throw cause;
      throw new RunnerError(3, "report-schema-invalid");
    }
    passedCaseCount += report.cases.filter((item) => item.result === "pass").length;
    failedCaseCount += report.cases.filter((item) => item.result === "fail").length;
    blockedCaseCount += report.cases.filter((item) => item.result === "blocked").length;
  }
  const accepted = passedCaseCount === expectedCaseCount && failedCaseCount === 0 && blockedCaseCount === 0 && missingCaseCount === 0 && forbiddenDataFindingCount === 0;
  const aggregate = aggregateReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite: "aggregate",
    mode: "aggregate",
    identity: preflight.identity,
    startedAt,
    completedAt: new Date().toISOString(),
    expectedCaseCount,
    passedCaseCount,
    failedCaseCount,
    blockedCaseCount,
    missingCaseCount,
    forbiddenDataFindingCount,
    reportSetSha256: hashEvidenceReportSet(reportDirectory),
    result: accepted ? "accepted" : "not-accepted",
  });
  writeJsonExclusive(reportDirectory, "aggregate.json", aggregate);
  return aggregate;
}

async function main() {
  try {
    const args = parseCliArguments(process.argv.slice(2));
    if (args.command === "preflight") {
      runPreflight(args.reportDirectory);
      process.stdout.write("mvp2x preflight: pass\n");
    } else if (args.command === "verify") {
      const report = await runVerify(args.reportDirectory, args.suite!, args.mode!);
      process.stdout.write(`mvp2x ${report.suite}/${report.mode}: ${report.result}\n`);
      if (report.result !== "pass") process.exitCode = 2;
    } else {
      const report = aggregateReports(args.reportDirectory);
      process.stdout.write(`mvp2x aggregate: ${report.result}\n`);
      if (report.result !== "accepted") process.exitCode = 2;
    }
  } catch (cause) {
    if (cause instanceof RunnerError) {
      process.stderr.write(`mvp2x: ${cause.code}\n`);
      process.exitCode = cause.exitCode;
      return;
    }
    process.stderr.write("mvp2x: internal\n");
    process.exitCode = 70;
  }
}

if (import.meta.main) await main();
