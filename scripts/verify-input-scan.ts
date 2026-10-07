import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, readlinkSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";

export function isDirectoryEvent(root: string, relativePath: string): boolean {
  try {
    return lstatSync(join(root, relativePath)).isDirectory();
  } catch {
    // A path that is already gone is left to the final scan, including events from another root.
    return true;
  }
}

/** Closed exclusion list. Tracked files still count, except root Git metadata. */
export const SCAN_EXCLUDED = [
  ".git",
  "node_modules",
  "target",
  "src-tauri/target",
  "dist",
  "dist-ssr",
  "coverage",
  "src-tauri/gen/schemas",
  "src-tauri/resources/bin",
  "tsconfig.tsbuildinfo",
  "tsconfig.node.tsbuildinfo",
];

const ROOT_TREES = new Set(["target", "dist", "dist-ssr", "coverage"]);

export type InputScan = { ok: true; digest: string } | { ok: false; reason: string };

export function eventPath(root: string, name: string | null | undefined): string {
  if (!name) return "";
  const path = name.split("\\").join("/");
  const base = root.split("\\").join("/");
  if (path === base) return "";
  if (path.startsWith(`${base}/`)) return path.slice(base.length + 1);
  if (path.startsWith("/")) return "";
  return path;
}

export function isScanExcluded(relativePath: string): boolean {
  const path = relativePath.split("\\").join("/");
  if (path === ".git" || path.startsWith(".git/")) return true;
  if (path.split("/").includes("node_modules")) return true;
  const [top] = path.split("/");
  if (top && ROOT_TREES.has(top)) return true;
  if (path === "src-tauri/target" || path.startsWith("src-tauri/target/")) return true;
  if (path === "src-tauri/gen/schemas" || path.startsWith("src-tauri/gen/schemas/")) return true;
  if (path === "src-tauri/resources/bin" || path.startsWith("src-tauri/resources/bin/"))
    return true;
  return path === "tsconfig.tsbuildinfo" || path === "tsconfig.node.tsbuildinfo";
}

export function scanRepository(root: string, tracked = trackedFiles(root)): InputScan {
  if (!tracked) return { ok: false, reason: "tracked files could not be listed" };
  const files: string[] = [];
  const walked = walk(root, root, tracked, files);
  if (!walked.ok) return walked;
  files.sort();
  const hash = createHash("sha256");
  for (const line of files) hash.update(line);
  return { ok: true, digest: hash.digest("hex") };
}

function trackedFiles(root: string): Set<string> | null {
  const result = Bun.spawnSync(["git", "ls-files", "-z"], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) return null;
  return new Set(
    new TextDecoder()
      .decode(result.stdout)
      .split("\0")
      .filter((path) => path.length > 0),
  );
}

function walk(
  root: string,
  directory: string,
  tracked: Set<string>,
  into: string[],
): InputScan | { ok: true; digest: "" } {
  let names: string[];
  try {
    names = readdirSync(directory);
  } catch (cause) {
    return unreadable(cause);
  }
  for (const name of names) {
    const full = join(directory, name);
    const path = relative(root, full).split(sep).join("/");
    if (path === ".git" || path.startsWith(".git/")) continue;
    let stat;
    try {
      stat = lstatSync(full);
    } catch (cause) {
      return unreadable(cause);
    }
    if (stat.isSymbolicLink()) {
      const linked = readLink(root, full);
      if (!linked.ok) return linked;
      if (include(path, tracked)) into.push(record(path, "symlink", false, linked.target));
      continue;
    }
    if (stat.isDirectory()) {
      if (isScanExcluded(path) && !hasTrackedChild(path, tracked)) continue;
      const nested = walk(root, full, tracked, into);
      if (!nested.ok) return nested;
      continue;
    }
    if (!stat.isFile()) return { ok: false, reason: "unsupported file type" };
    if (!include(path, tracked)) continue;
    try {
      into.push(record(path, "file", (stat.mode & 0o111) !== 0, readFileSync(full)));
    } catch (cause) {
      return unreadable(cause);
    }
  }
  return { ok: true, digest: "" };
}

function include(path: string, tracked: Set<string>): boolean {
  return !isScanExcluded(path) || tracked.has(path);
}

function hasTrackedChild(directory: string, tracked: Set<string>): boolean {
  const prefix = `${directory}/`;
  for (const path of tracked) if (path.startsWith(prefix)) return true;
  return false;
}

function readLink(root: string, full: string): { ok: true; target: string } | InputScan {
  let target: string;
  try {
    target = readlinkSync(full);
  } catch (cause) {
    return unreadable(cause);
  }
  const resolved = resolve(full, "..", target);
  const base = resolve(root);
  if (resolved !== base && !resolved.startsWith(base + sep)) {
    return { ok: false, reason: "symlink leaves the repository" };
  }
  return { ok: true, target };
}

function record(path: string, kind: string, executable: boolean, payload: Buffer | string): string {
  const hash = createHash("sha256");
  hash.update(kind);
  hash.update(executable ? "1" : "0");
  hash.update(payload);
  return `${path}\0${kind}\0${executable ? "1" : "0"}\0${hash.digest("hex")}\n`;
}

function unreadable(cause: unknown): InputScan {
  const code = (cause as NodeJS.ErrnoException).code;
  if (code === "ENOENT") return { ok: false, reason: "path disappeared while scanning" };
  return { ok: false, reason: "unreadable verification input" };
}
