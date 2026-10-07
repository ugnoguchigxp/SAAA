import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { readWorktreeChanges } from "./verify-affected-changes";
import {
  dependentsOf,
  parseManifests,
  previousManifestFiles,
  unionGraphs,
  type PackageGraph,
} from "./verify-affected-graph";
import {
  AVATAR_TESTS,
  mergeOwners,
  SELECTED_ALLOWLIST,
  type AffectedOwner,
  type Contract,
} from "./verify-affected-inputs";
import type { ChangeSet } from "./verify-affected-changes";
import { rustPackages, verificationPlan, type VerificationStep } from "./verify-plan";

export type AffectedLevel = "normal" | "advance" | "full";
export type AffectedMode = "shadow" | "selected";

export type AffectedPlan = {
  level: AffectedLevel;
  mode: AffectedMode;
  paths: string[];
  fallback: string | null;
  rustPackages: string[];
  typescriptTests: string[];
  contracts: Contract[];
  typescriptLint: true;
  reason: string;
  shadowRunsFullLevel: true;
  candidate: string[];
};

export function planAffected(input: {
  changes: ChangeSet;
  graph: PackageGraph;
  level: AffectedLevel;
  mode?: AffectedMode;
}): AffectedPlan {
  const mode = input.mode ?? "shadow";
  if (input.changes.fallback || input.graph.fallback) {
    return wide(
      input.level,
      mode,
      input.changes.paths,
      input.changes.fallback ?? input.graph.fallback ?? "unparsed input",
    );
  }
  const owners: AffectedOwner = mergeOwners(input.changes.paths);
  if (owners.fallback !== "none")
    return wide(input.level, mode, input.changes.paths, owners.reason);
  const rustPackages = dependentsOf(input.graph, owners.rustPackages);
  const candidate = [
    ...rustPackages.map((pkg) => `rust:${pkg}`),
    ...owners.typescriptTests.map((file) => `test:${file}`),
  ];
  return {
    level: input.level,
    mode,
    paths: input.changes.paths,
    fallback: null,
    rustPackages,
    typescriptTests: owners.typescriptTests,
    contracts: owners.contracts,
    typescriptLint: true,
    reason: owners.reason,
    shadowRunsFullLevel: true,
    candidate,
  };
}

function wide(
  level: AffectedLevel,
  mode: AffectedMode,
  paths: string[],
  reason: string,
): AffectedPlan {
  return {
    level,
    mode,
    paths,
    fallback: reason,
    rustPackages: [],
    typescriptTests: [],
    contracts: ["generated", "size", "quality", "ipc"],
    typescriptLint: true,
    reason,
    shadowRunsFullLevel: true,
    candidate: [],
  };
}

export function selectedAllowlistMatches(paths: string[]): boolean {
  return paths.length > 0 && paths.every((path) => SELECTED_ALLOWLIST.includes(path));
}

/** Narrow steps for the avatar allowlist. Full level is never reduced. */
export function selectedVerificationSteps(level: AffectedLevel, root: string): VerificationStep[] {
  if (level === "full") {
    throw new Error("selected full runs the whole full plan");
  }
  for (const file of AVATAR_TESTS) {
    if (!existsSync(join(root, file))) {
      throw new Error(`selected tests are missing: ${file}`);
    }
  }
  const staticSteps = verificationPlan(["--scope", "typescript"], root);
  if (level === "normal") return staticSteps;
  const build = verificationPlan(["build", "--scope", "typescript"], root);
  const tests = AVATAR_TESTS.map((file) => ({
    name: `frontend tests (${file})`,
    command: [process.execPath, "test", file],
  }));
  return dedupeSteps([...staticSteps, ...build, ...tests]);
}

export function explainAffected(root: string, level: AffectedLevel, mode: AffectedMode = "shadow") {
  const changes = readWorktreeChanges(root);
  const current = readManifests(root);
  if (current.fallback) {
    return planAffected({
      changes,
      graph: { packages: {}, fallback: current.fallback },
      level,
      mode,
    });
  }
  const deleted = changes.paths
    .filter((path) => path.endsWith("/Cargo.toml"))
    .map((path) => path.slice(0, -"/Cargo.toml".length));
  const directories = [...new Set([...Object.keys(current.files), ...deleted])];
  const previous = readPreviousManifests(root, directories);
  if (previous.fallback) {
    return planAffected({
      changes: { ...changes, fallback: changes.fallback ?? previous.fallback },
      graph: { packages: {}, fallback: previous.fallback },
      level,
      mode,
    });
  }
  return planAffected({
    changes,
    graph: unionGraphs(parseManifests(current.files), parseManifests(previous.files)),
    level,
    mode,
  });
}

function dedupeSteps(steps: VerificationStep[]): VerificationStep[] {
  const seen = new Set<string>();
  return steps.filter((step) => {
    const key = `${step.name}\0${step.command.join("\0")}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function readManifests(root: string): { files: Record<string, string>; fallback: string | null } {
  const files: Record<string, string> = {};
  try {
    for (const directory of rustPackages(root)) {
      const path = join(root, directory, "Cargo.toml");
      if (existsSync(path)) files[directory] = readFileSync(path, "utf8");
    }
  } catch (cause) {
    return { files: {}, fallback: cause instanceof Error ? cause.message : String(cause) };
  }
  return { files, fallback: null };
}

function readPreviousManifests(
  root: string,
  directories: string[],
): { files: Record<string, string>; fallback: string | null } {
  return previousManifestFiles(
    directories.map((directory) => ({
      directory,
      shown: gitShow(root, `HEAD:${directory}/Cargo.toml`),
    })),
  );
}

function gitShow(cwd: string, spec: string): string | "absent" | "error" {
  const result = Bun.spawnSync(["git", "show", spec], { cwd, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode === 0) return new TextDecoder().decode(result.stdout);
  const stderr = new TextDecoder().decode(result.stderr);
  if (stderr.includes("does not exist") || stderr.includes("exists on disk")) return "absent";
  return "error";
}
