/** Reverse dependencies from manifest text. Parse failure widens the gate. */

export type PackageGraph = {
  packages: Record<string, { normal: string[]; dev: string[]; build: string[] }>;
  fallback: string | null;
};

type Bucket = "normal" | "dev" | "build";

export function parseManifests(files: Record<string, string>): PackageGraph {
  const packages: PackageGraph["packages"] = {};
  try {
    for (const [directory, text] of Object.entries(files)) {
      const parsed = parseManifest(directory, text);
      if (parsed.fallback) return { packages: {}, fallback: parsed.fallback };
      packages[directory] = parsed.edges;
    }
  } catch (cause) {
    return { packages: {}, fallback: cause instanceof Error ? cause.message : String(cause) };
  }
  return { packages, fallback: null };
}

export function unionGraphs(current: PackageGraph, previous: PackageGraph): PackageGraph {
  if (current.fallback || previous.fallback) {
    return {
      packages: {},
      fallback: current.fallback ?? previous.fallback ?? "manifest could not be parsed",
    };
  }
  const packages: PackageGraph["packages"] = {};
  for (const graph of [current, previous]) {
    for (const [directory, deps] of Object.entries(graph.packages)) {
      const slot = packages[directory] ?? { normal: [], dev: [], build: [] };
      slot.normal = [...new Set([...slot.normal, ...deps.normal])];
      slot.dev = [...new Set([...slot.dev, ...deps.dev])];
      slot.build = [...new Set([...slot.build, ...deps.build])];
      packages[directory] = slot;
    }
  }
  return { packages, fallback: null };
}

export function dependentsOf(graph: PackageGraph, changed: string[]): string[] {
  if (graph.fallback) return [];
  const selected = new Set(changed);
  let grew = true;
  while (grew) {
    grew = false;
    for (const [directory, deps] of Object.entries(graph.packages)) {
      const all = [...deps.normal, ...deps.dev, ...deps.build];
      if (all.some((dep) => selected.has(dep)) && !selected.has(directory)) {
        selected.add(directory);
        grew = true;
      }
    }
  }
  return [...selected].sort();
}

/** Fold HEAD manifest reads. An unreadable parent manifest widens the gate. */
export function previousManifestFiles(
  entries: Array<{ directory: string; shown: string | "absent" | "error" }>,
): { files: Record<string, string>; fallback: string | null } {
  const files: Record<string, string> = {};
  for (const entry of entries) {
    if (entry.shown === "absent") continue;
    if (entry.shown === "error") {
      return { files: {}, fallback: "previous manifest could not be read" };
    }
    files[entry.directory] = entry.shown;
  }
  return { files, fallback: null };
}

function parseManifest(
  directory: string,
  text: string,
): { edges: { normal: string[]; dev: string[]; build: string[] }; fallback: string | null } {
  let document: unknown;
  try {
    document = Bun.TOML.parse(text);
  } catch (cause) {
    return {
      edges: { normal: [], dev: [], build: [] },
      fallback: cause instanceof Error ? cause.message : "manifest could not be parsed",
    };
  }
  if (!document || typeof document !== "object" || Array.isArray(document)) {
    return { edges: { normal: [], dev: [], build: [] }, fallback: "manifest could not be parsed" };
  }
  const edges = { normal: [] as string[], dev: [] as string[], build: [] as string[] };
  const root = document as Record<string, unknown>;
  const problem = collectEdges(root, directory, edges);
  if (problem) return { edges, fallback: problem };
  if (root.target !== undefined) {
    const targets = root.target;
    if (!targets || typeof targets !== "object" || Array.isArray(targets)) {
      return { edges, fallback: "unresolved target dependency" };
    }
    for (const target of Object.values(targets)) {
      if (!target || typeof target !== "object" || Array.isArray(target)) {
        return { edges, fallback: "unresolved target dependency" };
      }
      const targetProblem = collectEdges(target as Record<string, unknown>, directory, edges);
      if (targetProblem) return { edges, fallback: targetProblem };
    }
  }
  return { edges, fallback: null };
}

function collectEdges(
  table: Record<string, unknown>,
  directory: string,
  edges: { normal: string[]; dev: string[]; build: string[] },
): string | null {
  const groups: Array<[string, Bucket]> = [
    ["dependencies", "normal"],
    ["dev-dependencies", "dev"],
    ["build-dependencies", "build"],
  ];
  for (const [name, bucket] of groups) {
    const problem = readSpecs(table[name], directory, edges[bucket]);
    if (problem) return problem;
  }
  return null;
}

function readSpecs(value: unknown, directory: string, into: string[]): string | null {
  if (value === undefined) return null;
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return "manifest could not be parsed";
  }
  for (const spec of Object.values(value as Record<string, unknown>)) {
    if (typeof spec === "string") continue;
    if (!spec || typeof spec !== "object" || Array.isArray(spec)) {
      return "manifest could not be parsed";
    }
    const row = spec as Record<string, unknown>;
    if (row.workspace === true && typeof row.path !== "string") {
      return "unresolved workspace dependency";
    }
    if (typeof row.path === "string") {
      if (row.path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(row.path)) {
        return "unresolved absolute dependency";
      }
      into.push(normalizeJoin(directory, row.path));
    }
  }
  return null;
}

function normalizeJoin(directory: string, relative: string): string {
  const parts = `${directory}/${relative}`.split("/");
  const stack: string[] = [];
  for (const part of parts) {
    if (part === "" || part === ".") continue;
    if (part === "..") stack.pop();
    else stack.push(part);
  }
  return stack.join("/");
}
