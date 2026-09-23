import { existsSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { productionSource, walk } from "./module-size";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

type AuditRow = {
  path: string;
  unwrap: number;
  expect: number;
};

function posix(path: string): string {
  return relative(ROOT, path).split(sep).join("/");
}

function frozenSourcePaths(): Set<string> {
  const freezePath = join(ROOT, "critical-path-freeze.json");
  if (!existsSync(freezePath)) return new Set();
  const freeze = JSON.parse(readFileSync(freezePath, "utf8")) as {
    domains?: Record<string, { files?: Record<string, unknown> }>;
  };
  const paths = new Set<string>();
  for (const domain of Object.values(freeze.domains ?? {})) {
    for (const path of Object.keys(domain.files ?? {})) paths.add(path);
  }
  return paths;
}

function isExcludedPath(path: string): boolean {
  const lower = path.toLowerCase();
  if (lower.includes("test")) return true;
  if (lower.includes("tests/")) return true;
  return false;
}

function countOccurrences(source: string, needle: string): number {
  let count = 0;
  let index = 0;
  while (true) {
    const found = source.indexOf(needle, index);
    if (found === -1) return count;
    count += 1;
    index = found + needle.length;
  }
}

export function auditUnwraps(): AuditRow[] {
  const frozen = frozenSourcePaths();
  const rows: AuditRow[] = [];
  const rustRoot = join(ROOT, "src-tauri/src");
  if (!existsSync(rustRoot)) return rows;

  for (const absolute of walk(rustRoot)) {
    if (!absolute.endsWith(".rs")) continue;
    const path = posix(absolute);
    if (isExcludedPath(path) || frozen.has(path)) continue;
    const content = readFileSync(absolute, "utf8");
    const source = productionSource(content, path);
    if (!source) continue;
    const unwrap = countOccurrences(source, ".unwrap()");
    const expect = countOccurrences(source, ".expect(");
    if (unwrap === 0 && expect === 0) continue;
    rows.push({ path, unwrap, expect });
  }

  return rows.sort((left, right) => {
    if (right.unwrap !== left.unwrap) return right.unwrap - left.unwrap;
    if (right.expect !== left.expect) return right.expect - left.expect;
    return left.path.localeCompare(right.path);
  });
}

if (import.meta.main) {
  const rows = auditUnwraps();
  for (const row of rows) {
    console.log(`${row.path}\t${row.unwrap}\t${row.expect}`);
  }
  const totalUnwrap = rows.reduce((sum, row) => sum + row.unwrap, 0);
  const totalExpect = rows.reduce((sum, row) => sum + row.expect, 0);
  console.error(`files=${rows.length} unwrap=${totalUnwrap} expect=${totalExpect}`);
}
