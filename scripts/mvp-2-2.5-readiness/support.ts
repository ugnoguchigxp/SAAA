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

export const ROOT = fileURLToPath(new URL("../..", import.meta.url));
export const DEFAULT_BUNDLE = join(ROOT, "src-tauri/target/release/bundle/macos/SAAA.app");
export const DEFAULT_DEVELOPMENT_EXECUTABLE = join(
  ROOT,
  "src-tauri/target/debug/bundle/macos/SAAA.app/Contents/MacOS/saaa",
);
export const BUNDLE_IDENTIFIER = "com.saaa.desktop";
export const MAX_REPORT_BYTES = 256 * 1024;
export const APPLE_ROOT_SHA256 = new Set([
  "B0B1730ECBC7FF4505142C49F1295E6EDA6BCAED7E2C68C5BE91B5A11001F024",
  "C2B9B042DD57830E7D117DAC55AC8AE19407D38E41D88F3215BC3A890444A050",
  "63343ABFB89A6A03EBB57E9B3F5FA7BE7C4F5C756F3017B3A8C488C3653E9179",
]);

export {
  BUILD_CLASSES,
  MODES,
  REASON_CODES,
  RESULT_VALUES,
  RunnerError,
  SCHEMA_VERSION,
  SUITES,
  aggregateReportSchema,
  caseResultSchema,
  caseSpecs,
  identitySchema,
  observationSchema,
  preflightReportSchema,
  suiteReportSchema,
  type AggregateReport,
  type BuildClass,
  type CaseSpec,
  type CliArguments,
  type Identity,
  type MetricSpec,
  type Mode,
  type PreflightReport,
  type ReasonCode,
  type Result,
  type Suite,
  type SuiteReport,
} from "./schema.ts";

import {
  BUILD_CLASSES,
  MODES,
  REASON_CODES,
  RESULT_VALUES,
  RunnerError,
  SCHEMA_VERSION,
  SUITES,
  aggregateReportSchema,
  caseResultSchema,
  caseSpecs,
  identitySchema,
  observationSchema,
  preflightReportSchema,
  suiteReportSchema,
  type BuildClass,
  type CaseSpec,
  type CliArguments,
  type Identity,
  type MetricSpec,
  type Mode,
  type PreflightReport,
  type ReasonCode,
  type Result,
  type Suite,
  type SuiteReport,
} from "./schema.ts";

export function parseCliArguments(argv: string[]): CliArguments {
  const command = argv[0];
  if (command !== "preflight" && command !== "verify" && command !== "report")
    throw new RunnerError(64, "usage-error");
  let reportDirectory: string | undefined;
  let suite: Suite | undefined;
  let mode: Mode | undefined;
  for (let index = 1; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (
      flag === "--report-dir" &&
      reportDirectory === undefined &&
      value &&
      !value.startsWith("--")
    )
      reportDirectory = value;
    else if (flag === "--suite" && suite === undefined && SUITES.includes(value as Suite))
      suite = value as Suite;
    else if (flag === "--mode" && mode === undefined && MODES.includes(value as Mode))
      mode = value as Mode;
    else throw new RunnerError(64, "usage-error");
    index += 1;
  }
  if (!reportDirectory || !isAbsolute(reportDirectory)) throw new RunnerError(64, "usage-error");
  if ((command === "verify") !== Boolean(suite && mode)) throw new RunnerError(64, "usage-error");
  if (command === "verify") caseSpecs(suite!, mode!);
  return { command, reportDirectory, ...(suite ? { suite } : {}), ...(mode ? { mode } : {}) };
}

export function commandOutput(command: string, args: string[], cwd = ROOT): string {
  try {
    return execFileSync(command, args, {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  } catch {
    throw new RunnerError(3, "environment-invalid");
  }
}

export function requireDirectory(path: string, empty: boolean): string {
  if (!isAbsolute(path) || !existsSync(path)) throw new RunnerError(3, "environment-invalid");
  const info = lstatSync(path);
  if (!info.isDirectory() || info.isSymbolicLink()) throw new RunnerError(3, "environment-invalid");
  const canonical = realpathSync(path);
  if ((info.mode & 0o777) !== 0o700) throw new RunnerError(3, "environment-invalid");
  if (empty && readdirSync(canonical).length !== 0)
    throw new RunnerError(3, "report-directory-not-empty");
  return canonical;
}

export function isWithin(parent: string, candidate: string): boolean {
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

export function classifySigningDetails(
  details: string,
  certificateText: string,
  rootFingerprint: string,
): Identity["signingClass"] {
  if (/Signature=adhoc/i.test(details)) throw new RunnerError(3, "signature-invalid");
  if (!APPLE_ROOT_SHA256.has(rootFingerprint.replaceAll(":", "").toUpperCase()))
    throw new RunnerError(3, "signing-class-invalid");
  if (!/^TeamIdentifier=[A-Z0-9]{10}$/m.test(details))
    throw new RunnerError(3, "signing-class-invalid");
  if (
    /^Authority=Developer ID Application: .+$/m.test(details) &&
    /^Authority=Developer ID Certification Authority$/m.test(details) &&
    /1\.2\.840\.113635\.100\.6\.1\.13/.test(certificateText)
  )
    return "developer-id-application";
  if (
    /^Authority=Apple Development: .+$/m.test(details) &&
    /^Authority=Apple Worldwide Developer Relations Certification Authority$/m.test(details) &&
    /1\.2\.840\.113635\.100\.6\.1\.12/.test(certificateText)
  )
    return "apple-development";
  throw new RunnerError(3, "signing-class-invalid");
}

export function signingClass(bundlePath: string): Identity["signingClass"] {
  const verified = spawnSync("codesign", ["--verify", "--deep", "--strict", bundlePath], {
    encoding: "utf8",
  });
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
      .sort(
        (left, right) =>
          Number(left.slice("certificate".length)) - Number(right.slice("certificate".length)),
      )
      .map((name) => join(certificateDirectory, name));
    if (details.status !== 0 || certificates.length < 3)
      throw new RunnerError(3, "signature-invalid");
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
    const decoded = spawnSync(
      "openssl",
      ["x509", "-in", certificate, "-inform", "DER", "-noout", "-text"],
      { encoding: "utf8" },
    );
    if (decoded.status !== 0) throw new RunnerError(3, "signature-invalid");
    const root = spawnSync(
      "openssl",
      ["x509", "-in", rootCertificate, "-inform", "DER", "-noout", "-fingerprint", "-sha256"],
      { encoding: "utf8" },
    );
    const rootFingerprint = root.stdout?.match(/sha256 Fingerprint=([0-9A-F:]+)/i)?.[1];
    if (root.status !== 0 || !rootFingerprint) throw new RunnerError(3, "signature-invalid");
    return classifySigningDetails(output, decoded.stdout ?? "", rootFingerprint);
  } finally {
    rmSync(certificateDirectory, { recursive: true, force: true });
  }
}

export function bundleIdentity(bundlePath: string): Omit<Identity, "operator"> {
  const executable = join(bundlePath, "Contents/MacOS/saaa");
  if (!existsSync(executable)) throw new RunnerError(3, "bundle-missing");
  const identifier = commandOutput("/usr/libexec/PlistBuddy", [
    "-c",
    "Print:CFBundleIdentifier",
    join(bundlePath, "Contents/Info.plist"),
  ]);
  if (identifier !== BUNDLE_IDENTIFIER) throw new RunnerError(3, "bundle-identifier-invalid");
  const architecture = commandOutput("uname", ["-m"]);
  if (architecture !== "arm64" || !commandOutput("file", [executable]).includes("arm64"))
    throw new RunnerError(3, "architecture-invalid");
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

export function requiredEnvironmentDirectory(name: string, empty: boolean): string {
  const value = process.env[name];
  if (!value) throw new RunnerError(3, "environment-variable-missing");
  return requireDirectory(value, empty);
}

export function configuredBundlePath(): string {
  const bundlePath = process.env.SAAA_MVP2X_BUNDLE_PATH ?? DEFAULT_BUNDLE;
  if (!isAbsolute(bundlePath) || !existsSync(bundlePath) || lstatSync(bundlePath).isSymbolicLink())
    throw new RunnerError(3, "bundle-missing");
  return realpathSync(bundlePath);
}

export function configuredOperator(): string {
  const operator = process.env.SAAA_MVP2X_OPERATOR;
  if (!operator || !safeToken.safeParse(operator).success)
    throw new RunnerError(3, "operator-invalid");
  return operator;
}

export function isolatedEnvironmentDirectories(
  reportDirectory: string,
  appDataEmpty: boolean,
): { appDataDirectory: string; workspaceDirectory: string } {
  const appDataDirectory = requiredEnvironmentDirectory("SAAA_MVP2X_APP_DATA_DIR", appDataEmpty);
  const workspaceDirectory = requiredEnvironmentDirectory("SAAA_MVP2X_WORKSPACE_DIR", false);
  if (
    directoriesOverlap(ROOT, appDataDirectory) ||
    directoriesOverlap(ROOT, workspaceDirectory) ||
    directoriesOverlap(reportDirectory, appDataDirectory) ||
    directoriesOverlap(reportDirectory, workspaceDirectory) ||
    directoriesOverlap(appDataDirectory, workspaceDirectory)
  )
    throw new RunnerError(3, "environment-not-isolated");
  const normalAppData = join(homedir(), "Library/Application Support", BUNDLE_IDENTIFIER);
  if (resolve(appDataDirectory) === resolve(normalAppData))
    throw new RunnerError(3, "normal-app-data-refused");
  if (
    !existsSync(join(workspaceDirectory, ".git")) ||
    commandOutput("git", ["status", "--porcelain", "--untracked-files=all"], workspaceDirectory) !==
      ""
  )
    throw new RunnerError(3, "fixture-workspace-invalid");
  return { appDataDirectory, workspaceDirectory };
}

export function currentIdentity(): Identity {
  return { ...bundleIdentity(configuredBundlePath()), operator: configuredOperator() };
}

export function assertCurrentIdentity(expected: Identity): void {
  if (JSON.stringify(currentIdentity()) !== JSON.stringify(expected))
    throw new RunnerError(3, "identity-mismatch");
}

export function assertVerificationEnvironment(
  reportDirectory: string,
  preflight: PreflightReport,
): string {
  if (isWithin(ROOT, reportDirectory))
    throw new RunnerError(3, "report-directory-inside-repository");
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "")
    throw new RunnerError(3, "dirty-tree");
  const { appDataDirectory, workspaceDirectory } = isolatedEnvironmentDirectories(
    reportDirectory,
    false,
  );
  const databasePath = join(appDataDirectory, "saaa.sqlite3");
  if (!existsSync(databasePath)) throw new RunnerError(3, "dedicated-app-data-unused");
  const database = lstatSync(databasePath);
  if (!database.isFile() || database.isSymbolicLink() || database.nlink !== 1)
    throw new RunnerError(3, "dedicated-app-data-unused");
  const schemaTableCount = commandOutput("/usr/bin/sqlite3", [
    databasePath,
    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('settings_documents','conversations','runtime_runs');",
  ]);
  if (schemaTableCount !== "3") throw new RunnerError(3, "dedicated-app-data-unused");
  if (hashDirectory(workspaceDirectory, true) !== preflight.workspaceInitialSha256)
    throw new RunnerError(3, "fixture-workspace-invalid");
  assertCurrentIdentity(preflight.identity);
  return workspaceDirectory;
}

export function assertMetric(metric: MetricSpec, value: number): void {
  if (!Number.isFinite(value) || value < 0 || value > 10_000_000)
    throw new RunnerError(3, "observation-invalid");
  if (metric.exact !== undefined && value !== metric.exact)
    throw new RunnerError(2, "threshold-exceeded");
  if (metric.min !== undefined && value < metric.min)
    throw new RunnerError(2, "threshold-exceeded");
  if (metric.max !== undefined && value > metric.max)
    throw new RunnerError(2, "threshold-exceeded");
}

export const FORBIDDEN_KEYS = new Set([
  "credential",
  "token",
  "authorization",
  "endpoint",
  "host",
  "privateIp",
  "prompt",
  "response",
  "transcriptText",
  "audio",
  "workspacePath",
  "databasePath",
  "homeDirectory",
  "threadId",
  "turnId",
  "requestId",
  "windowTitle",
  "applicationIdentifier",
  "rawInputDuration",
]);
export const FORBIDDEN_TEXT =
  /(authorization\s*:|bearer\s+|https?:\/\/|\/Users\/|\/home\/|[A-Za-z]:\\|\b(?:\d{1,3}\.){3}\d{1,3}\b|\bssh\s+|\b[A-Za-z0-9._-]+@[A-Za-z0-9._-]+\b)/i;

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

export function writeJsonExclusive(
  reportDirectory: string,
  filename: string,
  value: unknown,
): void {
  if (!/^[a-z0-9.-]+\.json$/.test(filename) || forbiddenDataFindings(value) !== 0)
    throw new RunnerError(70, "redaction-failed");
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
    try {
      fsyncSync(directoryDescriptor);
    } finally {
      closeSync(directoryDescriptor);
    }
  } catch (cause) {
    if (targetLinked && existsSync(target)) unlinkSync(target);
    if (existsSync(temporary)) unlinkSync(temporary);
    if ((cause as NodeJS.ErrnoException).code === "EEXIST")
      throw new RunnerError(3, "report-overwrite-refused");
    throw new RunnerError(70, "atomic-write-failed");
  }
  unlinkSync(temporary);
}

export function readJson(path: string): unknown {
  const info = lstatSync(path);
  if (
    !info.isFile() ||
    info.isSymbolicLink() ||
    info.nlink !== 1 ||
    info.size > MAX_REPORT_BYTES ||
    (info.mode & 0o777) !== 0o600
  )
    throw new RunnerError(3, "report-file-invalid");
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new RunnerError(3, "report-schema-invalid");
  }
}

export function reportFilename(suite: Suite, mode: Mode): string {
  return `${suite}-${mode}.json`;
}

export function sameIdentity(left: Identity, right: Identity): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
