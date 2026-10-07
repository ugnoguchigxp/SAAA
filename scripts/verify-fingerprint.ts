import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, readlinkSync, realpathSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { isScanExcluded, type InputScan } from "./verify-input-scan";

export type InputFingerprint = { measured: boolean; value: string };

const RULES = [
  "scripts/verify.ts",
  "scripts/verify-plan.ts",
  "scripts/verify-affected-changes.ts",
  "scripts/verify-affected-inputs.ts",
  "scripts/verify-affected-graph.ts",
  "scripts/verify-affected-plan.ts",
  "scripts/verify-fingerprint.ts",
  "scripts/verify-input-scan.ts",
  "scripts/verify-report.ts",
  "scripts/verify-input-watch.ts",
  "scripts/verify-run.ts",
  "rust-toolchain.toml",
];

const UNMEASURED: InputFingerprint = { measured: false, value: "" };

export function trackedInputFiles(root: string): Set<string> | null {
  const bytes = gitBytes(root, ["ls-files", "-z"]);
  return bytes ? new Set(bytes.toString("utf8").split("\0").filter(Boolean)) : null;
}

/** Walk all verification inputs, including ignored files; never follow a symlink outside root. */
export function scanVerificationInputs(root: string): InputScan {
  const tracked = trackedInputFiles(root);
  if (!tracked) return { ok: false, reason: "tracked files could not be listed" };
  const trackedDirectories = new Set<string>();
  for (const path of tracked) {
    let end = path.lastIndexOf("/");
    while (end > 0) {
      trackedDirectories.add(path.slice(0, end));
      end = path.lastIndexOf("/", end - 1);
    }
  }
  try {
    const base = realpathSync(root);
    const entries: string[] = [];
    const inside = (path: string) => path === base || path.startsWith(base + sep);
    const visit = (directory: string) => {
      for (const name of readdirSync(directory)) {
        const full = join(directory, name);
        const path = relative(root, full).split(sep).join("/");
        if (path === ".git" || path.startsWith(".git/")) continue;
        const included = !isScanExcluded(path) || tracked.has(path);
        const children = trackedDirectories.has(path);
        if (!included && !children) continue;
        const stat = lstatSync(full);
        if (stat.isDirectory()) {
          visit(full);
          continue;
        }
        if (!included) continue;
        let kind: string;
        let bytes: Buffer | string;
        if (stat.isSymbolicLink()) {
          kind = "symlink";
          bytes = readlinkSync(full);
          if (!inside(resolve(realpathSync(dirname(full)), bytes)) || !inside(realpathSync(full)))
            throw new Error("symlink leaves the repository");
        } else if (stat.isFile()) {
          kind = "file";
          bytes = readFileSync(full);
        } else throw new Error("unsupported verification input type");
        const content = createHash("sha256").update(bytes).digest("hex");
        entries.push(JSON.stringify([path, kind, stat.mode & 0o111, content]));
      }
    };
    visit(root);
    const digest = createHash("sha256").update(entries.sort().join("\n")).digest("hex");
    return { ok: true, digest };
  } catch {
    // Do not expose file contents, link targets, or secret values in reports.
    return { ok: false, reason: "verification input could not be scanned safely" };
  }
}

/**
 * Snapshot of HEAD, the index diff, dirty worktree bytes, and verify rules.
 * A matching hash does not prove every input was unchanged: ignored files,
 * the environment, and files that appear after the snapshot are outside it.
 */
export function captureInputFingerprint(cwd: string): InputFingerprint {
  const head = gitBytes(cwd, ["rev-parse", "HEAD"]);
  const staged = gitBytes(cwd, ["diff", "--cached"]);
  const status = gitBytes(cwd, ["status", "--porcelain=v1", "--untracked-files=all", "-z"]);
  if (!head || !staged || !status) return UNMEASURED;
  const hash = createHash("sha256");
  hash.update(head);
  hash.update(staged);
  hash.update(status);
  for (const path of pathsFromPorcelain(status)) {
    try {
      const bytes = readFileSync(join(cwd, path));
      hash.update(path);
      hash.update("\0");
      hash.update(bytes);
      hash.update("\0");
    } catch (cause) {
      if (isMissing(cause)) continue;
      return UNMEASURED;
    }
  }
  for (const path of RULES) {
    hash.update(path);
    hash.update("\0");
    try {
      hash.update(readFileSync(join(cwd, path)));
    } catch {
      hash.update(`missing:${path}`);
    }
    hash.update("\0");
  }
  return { measured: true, value: hash.digest("hex") };
}

export function staleDecision(
  before: InputFingerprint,
  after: InputFingerprint,
  attempt: number,
): "ok" | "retry" | "stale" | "unmeasured" {
  if (!before.measured || !after.measured) return "unmeasured";
  if (before.value === after.value) return "ok";
  return attempt < 2 ? "retry" : "stale";
}

function splitZero(buffer: Buffer): Buffer[] {
  const parts: Buffer[] = [];
  let start = 0;
  for (let index = 0; index < buffer.length; index += 1) {
    if (buffer[index] !== 0) continue;
    if (index > start) parts.push(buffer.subarray(start, index));
    start = index + 1;
  }
  if (start < buffer.length) parts.push(buffer.subarray(start));
  return parts;
}

function isMissing(cause: unknown): boolean {
  return typeof cause === "object" && cause !== null && "code" in cause && cause.code === "ENOENT";
}

function gitBytes(cwd: string, args: string[]): Buffer | null {
  const result = Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) return null;
  return Buffer.from(result.stdout);
}

/** Porcelain v1 `-z` puts the rename or copy destination in the status record. The next field is the old path. */
export function pathsFromPorcelain(status: Buffer): string[] {
  const parts = splitZero(status);
  const paths: string[] = [];
  for (let index = 0; index < parts.length; index += 1) {
    const entry = parts[index]?.toString("utf8") ?? "";
    if (entry.length < 4) continue;
    const code = entry.slice(0, 2);
    const path = entry.slice(3);
    if (code.includes("R") || code.includes("C")) {
      index += 1;
      paths.push(path);
      continue;
    }
    paths.push(path);
  }
  return paths;
}
