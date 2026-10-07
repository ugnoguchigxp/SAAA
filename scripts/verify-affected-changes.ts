/** Changed paths from git name-status text. This module does not run git. */

export type ChangeReason =
  | "staged"
  | "unstaged"
  | "untracked"
  | "commit"
  | "rename-old"
  | "rename-new"
  | "deleted";

export type ChangeSet = {
  paths: string[];
  reasons: Record<string, ChangeReason[]>;
  fallback: string | null;
};

export function collectChanges(input: {
  staged: string;
  unstaged: string;
  untracked: string[];
  commits?: string;
  baseKnown: boolean;
  gitFailed?: boolean;
}): ChangeSet {
  if (input.gitFailed || (input.commits !== undefined && !input.baseKnown)) {
    return {
      paths: [],
      reasons: {},
      fallback: input.gitFailed ? "git status could not be read" : "comparison base is unknown",
    };
  }
  const reasons = new Map<string, ChangeReason[]>();
  const add = (path: string, reason: ChangeReason) => {
    const current = reasons.get(path) ?? [];
    if (!current.includes(reason)) current.push(reason);
    reasons.set(path, current);
  };
  for (const path of parseNameStatus(input.staged, "staged")) add(path.path, path.reason);
  for (const path of parseNameStatus(input.unstaged, "unstaged")) add(path.path, path.reason);
  for (const path of input.untracked) add(path, "untracked");
  if (input.commits) {
    for (const path of parseNameStatus(input.commits, "commit")) add(path.path, path.reason);
  }
  return {
    paths: [...reasons.keys()].sort(),
    reasons: Object.fromEntries(reasons),
    fallback: null,
  };
}

function parseNameStatus(
  text: string,
  kind: "staged" | "unstaged" | "commit",
): Array<{ path: string; reason: ChangeReason }> {
  const found: Array<{ path: string; reason: ChangeReason }> = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    const parts = line.split("\t");
    const status = parts[0] ?? "";
    if (status.startsWith("R") || status.startsWith("C")) {
      const oldPath = parts[1];
      const newPath = parts[2];
      if (oldPath) found.push({ path: unquoteGitPath(oldPath), reason: "rename-old" });
      if (newPath) found.push({ path: unquoteGitPath(newPath), reason: "rename-new" });
      continue;
    }
    const path = parts[1];
    if (!path) continue;
    const decoded = unquoteGitPath(path);
    if (status.startsWith("D")) found.push({ path: decoded, reason: "deleted" });
    else found.push({ path: decoded, reason: kind });
  }
  return found;
}

/** Git quotes paths that contain spaces or non-ASCII. Octal escapes are raw bytes. */
export function unquoteGitPath(path: string): string {
  if (!(path.startsWith('"') && path.endsWith('"') && path.length >= 2)) return path;
  const bytes: number[] = [];
  const body = path.slice(1, -1);
  for (let index = 0; index < body.length; index += 1) {
    const char = body[index];
    if (char !== "\\") {
      if (char !== undefined) bytes.push(char.charCodeAt(0));
      continue;
    }
    const next = body[index + 1];
    index += 1;
    if (next === "n") bytes.push(10);
    else if (next === "t") bytes.push(9);
    else if (next === "\\" || next === '"') bytes.push(next.charCodeAt(0));
    else if (next !== undefined && next >= "0" && next <= "7") {
      let octal = next;
      for (let extra = 0; extra < 2; extra += 1) {
        const digit = body[index + 1];
        if (digit === undefined || digit < "0" || digit > "7") break;
        octal += digit;
        index += 1;
      }
      bytes.push(Number.parseInt(octal, 8));
    } else if (next !== undefined) bytes.push(next.charCodeAt(0));
  }
  return new TextDecoder().decode(Uint8Array.from(bytes));
}

/** NUL-separated git name-status. A truncated or non-UTF-8 record widens the gate. */
export function collectNulChanges(input: {
  staged: Uint8Array;
  unstaged: Uint8Array;
  untracked: Uint8Array;
  baseKnown: boolean;
  gitFailed?: boolean;
}): ChangeSet {
  if (input.gitFailed || !input.baseKnown) {
    return {
      paths: [],
      reasons: {},
      fallback: input.gitFailed ? "git status could not be read" : "comparison base is unknown",
    };
  }
  const staged = parseNulNameStatus(input.staged, "staged");
  const unstaged = parseNulNameStatus(input.unstaged, "unstaged");
  const untracked = splitNul(input.untracked);
  if (!staged || !unstaged || !untracked) {
    return { paths: [], reasons: {}, fallback: "git status could not be read" };
  }
  const reasons = new Map<string, ChangeReason[]>();
  const add = (path: string, reason: ChangeReason) => {
    const current = reasons.get(path) ?? [];
    if (!current.includes(reason)) current.push(reason);
    reasons.set(path, current);
  };
  for (const entry of staged) add(entry.path, entry.reason);
  for (const entry of unstaged) add(entry.path, entry.reason);
  for (const path of untracked) add(path, "untracked");
  return {
    paths: [...reasons.keys()].sort(),
    reasons: Object.fromEntries(reasons),
    fallback: null,
  };
}

function splitNul(bytes: Uint8Array): string[] | null {
  if (bytes.length === 0) return [];
  if (bytes[bytes.length - 1] !== 0) return null;
  const parts: string[] = [];
  let start = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    if (bytes[index] !== 0) continue;
    try {
      parts.push(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(start, index)));
    } catch {
      return null;
    }
    start = index + 1;
  }
  return parts;
}

function parseNulNameStatus(
  bytes: Uint8Array,
  kind: "staged" | "unstaged" | "commit",
): Array<{ path: string; reason: ChangeReason }> | null {
  const fields = splitNul(bytes);
  if (!fields) return null;
  const found: Array<{ path: string; reason: ChangeReason }> = [];
  for (let index = 0; index < fields.length;) {
    const status = fields[index];
    const path = fields[index + 1];
    if (!status || path === undefined) return null;
    index += 2;
    if (status.startsWith("R") || status.startsWith("C")) {
      const renamed = fields[index];
      if (renamed === undefined) return null;
      index += 1;
      found.push({ path, reason: "rename-old" });
      found.push({ path: renamed, reason: "rename-new" });
      continue;
    }
    found.push({ path, reason: status.startsWith("D") ? "deleted" : kind });
  }
  return found;
}

export function readWorktreeChanges(cwd: string): ChangeSet {
  const run = (args: string[]) => {
    const result = Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) return null;
    return new Uint8Array(result.stdout);
  };
  const staged = run(["diff", "--cached", "-z", "--name-status"]);
  const unstaged = run(["diff", "-z", "--name-status"]);
  const untracked = run(["ls-files", "-z", "--others", "--exclude-standard"]);
  if (!staged || !unstaged || !untracked) {
    return collectNulChanges({
      staged: new Uint8Array(),
      unstaged: new Uint8Array(),
      untracked: new Uint8Array(),
      baseKnown: false,
      gitFailed: true,
    });
  }
  return collectNulChanges({ staged, unstaged, untracked, baseKnown: true });
}
