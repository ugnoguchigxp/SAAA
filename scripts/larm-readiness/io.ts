import { createHash } from "node:crypto";
import {
  closeSync,
  constants as fsConstants,
  existsSync,
  fstatSync,
  lstatSync,
  openSync,
  readSync,
  readdirSync,
  realpathSync,
  statSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import {
  COMPILED_LARM_CONTRACT_COMMIT,
  FROZEN_LARM_CONTRACT_COMMIT,
  MAX_MANIFEST_BYTES,
  REPORT_FILENAMES,
  ROOT,
  RunnerError,
  commit,
  manifestSchema,
  revision,
  type CanaryManifest,
  type CliArguments,
  validateHeaderCredential,
  validateNumericLoopbackOrigin,
} from "./schema.ts";

export function modeBits(value: number): number {
  return value & 0o777;
}

export function assertNoSymlinkComponents(target: string): void {
  const absolute = resolve(target);
  const root = absolute.startsWith(sep) ? sep : absolute.slice(0, absolute.indexOf(sep) + 1);
  const parts = absolute.slice(root.length).split(sep).filter(Boolean);
  let current = root;
  for (const part of parts) {
    current = join(current, part);
    if (!existsSync(current)) break;
    if (lstatSync(current).isSymbolicLink()) throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function assertCurrentOwner(target: string): void {
  if (typeof process.getuid === "function" && statSync(target).uid !== process.getuid()) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function canonicalDirectory(target: string, expectedMode: number): string {
  try {
    if (!isAbsolute(target) || resolve(target) !== target) throw new RunnerError(3, "environment-invalid", "blocked");
    assertNoSymlinkComponents(target);
    const info = lstatSync(target);
    if (!info.isDirectory() || info.isSymbolicLink() || modeBits(info.mode) !== expectedMode) {
      throw new RunnerError(3, "environment-invalid", "blocked");
    }
    assertCurrentOwner(target);
    const canonical = realpathSync(target);
    if (canonical !== target) throw new RunnerError(3, "environment-invalid", "blocked");
    return canonical;
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function isAncestorOrSame(left: string, right: string): boolean {
  const value = relative(left, right);
  return value === "" || (!value.startsWith(`..${sep}`) && value !== "..");
}

export function assertSeparated(paths: string[]): void {
  for (let left = 0; left < paths.length; left += 1) {
    for (let right = left + 1; right < paths.length; right += 1) {
      if (isAncestorOrSame(paths[left]!, paths[right]!) || isAncestorOrSame(paths[right]!, paths[left]!)) {
        throw new RunnerError(3, "environment-invalid", "blocked");
      }
    }
  }
}

export function validateReportDirectory(target: string, command: CliArguments["command"], duration?: "30m" | "2h"): string {
  try {
    const canonical = canonicalDirectory(target, 0o700);
    const entries = readdirSync(canonical).sort();
    const expected = command === "preflight"
      ? []
      : command === "canary"
        ? [REPORT_FILENAMES.preflight]
        : command === "soak" && duration === "30m"
          ? [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional]
          : command === "soak" && duration === "2h"
            ? [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional, REPORT_FILENAMES["soak-30m"]]
            : [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional, REPORT_FILENAMES["soak-30m"], REPORT_FILENAMES["soak-2h"]];
    if (entries.length !== expected.length || entries.some((entry, index) => entry !== [...expected].sort()[index])) {
      throw new RunnerError(3, "environment-invalid", "blocked");
    }
    for (const entry of entries) {
      const filename = join(canonical, entry);
      const info = lstatSync(filename);
      if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || modeBits(info.mode) !== 0o600) {
        throw new RunnerError(3, "environment-invalid", "blocked");
      }
      assertCurrentOwner(filename);
    }
    return canonical;
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export interface StableFileIdentity {
  dev: number;
  ino: number;
  size: number;
  nlink: number;
  mode: number;
  mtimeMs: number;
  ctimeMs: number;
}

export function stableFileIdentity(info: ReturnType<typeof fstatSync>): StableFileIdentity {
  return {
    dev: info.dev,
    ino: info.ino,
    size: info.size,
    nlink: info.nlink,
    mode: info.mode,
    mtimeMs: info.mtimeMs,
    ctimeMs: info.ctimeMs,
  };
}

export function sameFileIdentity(left: StableFileIdentity, right: StableFileIdentity): boolean {
  return (Object.keys(left) as Array<keyof StableFileIdentity>).every((key) => left[key] === right[key]);
}

export function sameFileObject(left: StableFileIdentity, right: StableFileIdentity): boolean {
  return left.dev === right.dev
    && left.ino === right.ino
    && left.nlink === right.nlink
    && left.mode === right.mode;
}

export function readBoundedRegularFile(
  filename: string,
  maximumBytes: number,
  failure: () => RunnerError,
  expectedMode?: number,
): { bytes: Buffer; identity: StableFileIdentity } {
  let descriptor: number | undefined;
  try {
    assertNoSymlinkComponents(filename);
    descriptor = openSync(filename, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const beforeInfo = fstatSync(descriptor);
    const before = stableFileIdentity(beforeInfo);
    if (
      !beforeInfo.isFile()
      || beforeInfo.isSymbolicLink()
      || before.nlink !== 1
      || before.size > maximumBytes
      || (expectedMode !== undefined && modeBits(before.mode) !== expectedMode)
      || (typeof process.getuid === "function" && beforeInfo.uid !== process.getuid())
    ) {
      throw failure();
    }
    const chunks: Buffer[] = [];
    let total = 0;
    while (true) {
      const chunk = Buffer.allocUnsafe(Math.min(64 * 1_024, maximumBytes + 1 - total));
      const count = readSync(descriptor, chunk, 0, chunk.length, null);
      if (count === 0) break;
      total += count;
      if (total > maximumBytes) throw failure();
      chunks.push(chunk.subarray(0, count));
    }
    const afterInfo = fstatSync(descriptor);
    const after = stableFileIdentity(afterInfo);
    if (!sameFileIdentity(before, after) || total !== before.size) throw failure();
    return { bytes: Buffer.concat(chunks, total), identity: before };
  } catch {
    throw failure();
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

export function parseJsonBytes(bytes: Buffer, failure: () => RunnerError): unknown {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return JSON.parse(text);
  } catch {
    throw failure();
  }
}

export function validateRollbackEndpoint(provider: CanaryManifest["rollbackProvider"]): void {
  let endpoint: URL;
  try {
    endpoint = new URL(provider.endpoint);
  } catch {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  if (endpoint.username || endpoint.password) throw new RunnerError(3, "environment-invalid", "blocked");
  if (provider.location === "cloud") {
    if (endpoint.protocol !== "https:") throw new RunnerError(3, "environment-invalid", "blocked");
    return;
  }
  const hostname = endpoint.hostname.toLowerCase();
  const octets = hostname.split(".").map(Number);
  const privateIpv4 = octets.length === 4
    && octets.every((part) => Number.isInteger(part) && part >= 0 && part <= 255)
    && (octets[0] === 10
      || octets[0] === 127
      || (octets[0] === 172 && octets[1]! >= 16 && octets[1]! <= 31)
      || (octets[0] === 192 && octets[1] === 168));
  if (endpoint.protocol !== "http:" || !(hostname === "localhost" || hostname === "[::1]" || privateIpv4)) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function loadManifestUnchecked(filename: string, phase: "preflight" | "functional" | "later", repositoryRoot = ROOT): { manifest: CanaryManifest; sha256: string; canonicalFile: string } {
  if (!isAbsolute(filename) || resolve(filename) !== filename) throw new RunnerError(3, "environment-invalid", "blocked");
  assertNoSymlinkComponents(filename);
  const info = lstatSync(filename);
  if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || info.size > MAX_MANIFEST_BYTES || modeBits(info.mode) !== 0o600) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  assertCurrentOwner(filename);
  assertCurrentOwner(dirname(filename));
  const failure = () => new RunnerError(3, "environment-invalid", "blocked");
  const { bytes } = readBoundedRegularFile(filename, MAX_MANIFEST_BYTES, failure, 0o600);
  const parsed = manifestSchema.safeParse(parseJsonBytes(bytes, failure));
  if (!parsed.success) throw new RunnerError(3, "environment-invalid", "blocked");
  const manifest = parsed.data;
  validateNumericLoopbackOrigin(manifest.larmProvider.baseUrl);
  validateRollbackEndpoint(manifest.rollbackProvider);
  const canonicalFile = realpathSync(filename);
  if (canonicalFile !== filename) throw new RunnerError(3, "environment-invalid", "blocked");
  const canonicalData = canonicalDirectory(manifest.dataDirectory, 0o700);
  const home = realpathSync(homedir());
  const filesystemRoot = realpathSync(sep);
  const repository = realpathSync(repositoryRoot);
  const normalDataPath = join(home, "Library", "Application Support", "com.saaa.desktop");
  const normalData = existsSync(normalDataPath) ? realpathSync(normalDataPath) : resolve(normalDataPath);
  if (
    canonicalData === home
    || canonicalData === filesystemRoot
    || isAncestorOrSame(canonicalData, repository)
    || isAncestorOrSame(canonicalData, normalData)
    || isAncestorOrSame(normalData, canonicalData)
  ) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const dataEntries = readdirSync(canonicalData);
  if (phase === "preflight" || phase === "functional" ? dataEntries.length !== 0 : !dataEntries.includes("saaa.sqlite3")) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  return { manifest: { ...manifest, dataDirectory: canonicalData }, sha256: createHash("sha256").update(bytes).digest("hex"), canonicalFile };
}

export function loadManifest(filename: string, phase: "preflight" | "functional" | "later", repositoryRoot = ROOT): { manifest: CanaryManifest; sha256: string; canonicalFile: string } {
  try {
    return loadManifestUnchecked(filename, phase, repositoryRoot);
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function assertManifestSeparation(manifestFile: string, dataDirectory: string, reportDirectory: string): void {
  assertSeparated([realpathSync(ROOT), manifestFile, dataDirectory, reportDirectory]);
}

export interface ValidatedEnvironment {
  baseUrl: string;
  deployedCommit: string;
  deploymentRevision: string;
  token: string;
  rollbackCredential?: string;
  reportDirectory: string;
  dataDirectory: string;
  manifest: CanaryManifest;
  manifestSha256: string;
  manifestFile: string;
}

export const CALLER_FORBIDDEN_VARIABLES = [
  "SAAA_SMOKE_MARKER_ID",
  "SAAA_SMOKE_EXERCISE_SITUATION",
  "SAAA_LARM_CANARY_RESULT_FILE",
  "SAAA_LARM_CANARY_ARTIFACT_SHA256",
  "SAAA_LARM_CANARY_MANIFEST_SHA256",
  "SAAA_LARM_CANARY_SAAA_COMMIT",
  "SAAA_LARM_CANARY_METRICS_SCOPE",
] as const;

export function validateLiveEnvironmentUnchecked(
  reportDirectory: string,
  phase: "preflight" | "functional" | "later",
  environment: Record<string, string | undefined> = process.env,
): ValidatedEnvironment {
  if (FROZEN_LARM_CONTRACT_COMMIT === null) throw new RunnerError(3, "gate-missing", "blocked");
  if (environment.SAAA_LARM_CANARY !== "1" || environment.SAAA_LARM_ENABLED !== "1") {
    throw new RunnerError(3, "gate-missing", "blocked");
  }
  if (CALLER_FORBIDDEN_VARIABLES.some((key) => environment[key] !== undefined)) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const token = validateHeaderCredential(environment.LARM_API_TOKEN);
  const baseUrl = validateNumericLoopbackOrigin(environment.SAAA_LARM_CANARY_BASE_URL ?? "");
  const deployedCommit = commit.safeParse(environment.SAAA_LARM_DEPLOYED_COMMIT);
  const deploymentRevision = revision.safeParse(environment.SAAA_LARM_DEPLOYMENT_REVISION);
  const manifestFilename = environment.SAAA_LARM_CANARY_MANIFEST_FILE;
  const environmentReportDirectory = environment.SAAA_LARM_CANARY_REPORT_DIR;
  const environmentDataDirectory = environment.SAAA_SMOKE_DATA_DIR;
  if (!deployedCommit.success || !deploymentRevision.success || manifestFilename === undefined || environmentReportDirectory === undefined || environmentDataDirectory === undefined) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const canonicalReport = realpathSync(reportDirectory);
  if (realpathSync(environmentReportDirectory) !== canonicalReport) throw new RunnerError(3, "environment-invalid", "blocked");
  const loaded = loadManifest(manifestFilename, phase);
  if (
    loaded.manifest.saaaCommit !== gitCommitSync()
    || loaded.manifest.larmContractCommit !== FROZEN_LARM_CONTRACT_COMMIT
    || loaded.manifest.larmContractCommit !== COMPILED_LARM_CONTRACT_COMMIT
    || loaded.manifest.larmContractCommit !== deployedCommit.data
    || loaded.manifest.deploymentRevision !== deploymentRevision.data
    || loaded.manifest.larmProvider.baseUrl !== baseUrl
    || loaded.manifest.dataDirectory !== realpathSync(environmentDataDirectory)
  ) {
    throw new RunnerError(2, "contract-mismatch", "failed");
  }
  assertManifestSeparation(loaded.canonicalFile, loaded.manifest.dataDirectory, canonicalReport);
  const rawRollbackCredential = environment[loaded.manifest.rollbackProvider.credentialEnv];
  if (loaded.manifest.rollbackProvider.credentialRequired && rawRollbackCredential === undefined) {
    throw new RunnerError(3, "gate-missing", "blocked");
  }
  const rollbackCredential = rawRollbackCredential === undefined
    ? undefined
    : validateHeaderCredential(rawRollbackCredential);
  return {
    baseUrl,
    deployedCommit: deployedCommit.data,
    deploymentRevision: deploymentRevision.data,
    token,
    ...(rollbackCredential === undefined ? {} : { rollbackCredential }),
    reportDirectory: canonicalReport,
    dataDirectory: loaded.manifest.dataDirectory,
    manifest: loaded.manifest,
    manifestSha256: loaded.sha256,
    manifestFile: loaded.canonicalFile,
  };
}

export function validateLiveEnvironment(
  reportDirectory: string,
  phase: "preflight" | "functional" | "later",
  environment: Record<string, string | undefined> = process.env,
): ValidatedEnvironment {
  try {
    return validateLiveEnvironmentUnchecked(reportDirectory, phase, environment);
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

export function gitCommitSync(): string {
  const result = Bun.spawnSync(["git", "rev-parse", "HEAD"], { cwd: ROOT, stdout: "pipe", stderr: "pipe", env: buildEnvironment(process.env) });
  const value = result.stdout.toString().trim();
  if (result.exitCode !== 0 || !commit.safeParse(value).success) throw new RunnerError(70, "internal", "failed");
  return value;
}

export function assertCleanRepository(): string {
  const before = gitCommitSync();
  const result = Bun.spawnSync(["git", "status", "--porcelain=v1", "--untracked-files=all"], { cwd: ROOT, stdout: "pipe", stderr: "pipe", env: buildEnvironment(process.env) });
  if (result.exitCode !== 0) throw new RunnerError(70, "internal", "failed");
  if (result.stdout.byteLength !== 0) throw new RunnerError(3, "environment-invalid", "blocked");
  return before;
}

export const BUILD_ENVIRONMENT_KEYS = [
  "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "LOGNAME", "LANG", "LC_ALL",
  "CARGO_HOME", "RUSTUP_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE",
  "DEVELOPER_DIR", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET",
] as const;
export function buildEnvironment(environment: Record<string, string | undefined>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of BUILD_ENVIRONMENT_KEYS) if (environment[key] !== undefined) result[key] = environment[key]!;
  return result;
}

export const RUST_BASE_ENVIRONMENT = ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "CARGO_HOME", "RUSTUP_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE"] as const;
export function rustChildEnvironment(environment: Record<string, string | undefined>, internal: {
  resultFile: string;
  artifactSha256: string;
  manifestSha256: string;
  metricsScope: CanaryManifest["metricsScope"];
  saaaCommit?: string;
}): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of RUST_BASE_ENVIRONMENT) if (environment[key] !== undefined) result[key] = environment[key]!;
  for (const key of ["SAAA_LARM_CANARY", "SAAA_LARM_ENABLED", "SAAA_LARM_CANARY_BASE_URL", "SAAA_LARM_DEPLOYED_COMMIT", "SAAA_LARM_DEPLOYMENT_REVISION", "LARM_API_TOKEN"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  result.SAAA_LARM_CANARY_RESULT_FILE = internal.resultFile;
  result.SAAA_LARM_CANARY_ARTIFACT_SHA256 = internal.artifactSha256;
  result.SAAA_LARM_CANARY_MANIFEST_SHA256 = internal.manifestSha256;
  result.SAAA_LARM_CANARY_METRICS_SCOPE = internal.metricsScope;
  if (internal.saaaCommit !== undefined) result.SAAA_LARM_CANARY_SAAA_COMMIT = internal.saaaCommit;
  return result;
}

export function appChildEnvironment(environment: Record<string, string | undefined>, internal: {
  enabled: boolean;
  markerId: string;
  dataDirectory: string;
}): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "LOGNAME", "LANG", "LC_ALL", "DISPLAY", "XDG_RUNTIME_DIR"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  for (const key of ["SAAA_LARM_CANARY", "LARM_API_TOKEN", "SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  result.SAAA_LARM_ENABLED = internal.enabled ? "1" : "0";
  result.SAAA_SMOKE_MARKER_ID = internal.markerId;
  result.SAAA_SMOKE_DATA_DIR = internal.dataDirectory;
  return result;
}

export class ForbiddenDataScanner {
  private overlap = Buffer.alloc(0);
  private failed = false;
  private readonly exact: Buffer[];

  constructor(values: Array<string | undefined>) {
    this.exact = values
      .filter((value): value is string => value !== undefined && Buffer.byteLength(value) > 0)
      .map((value) => Buffer.from(value));
  }

  scan(chunk: Uint8Array): boolean {
    const bytes = Buffer.concat([this.overlap, Buffer.from(chunk)]);
    const text = bytes.toString("utf8");
    this.failed ||= this.exact.some((needle) => bytes.includes(needle))
      || /authorization\s*:/i.test(text)
      || /bearer\s+[\x21-\x7e]+/i.test(text)
      || /(?:allocation|operation|request|conversation|runtime[_ -]?run)[_ -]?id["']?\s*[=:]/i.test(text);
    this.overlap = bytes.subarray(Math.max(0, bytes.length - 4_095));
    return this.failed;
  }

  get detected(): boolean {
    return this.failed;
  }

  fork(): ForbiddenDataScanner {
    return new ForbiddenDataScanner(this.exact.map((value) => value.toString("utf8")));
  }
}
