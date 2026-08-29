import { existsSync, lstatSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const BASELINE_PATH = join(ROOT, "scripts/module-size-baseline.json");
const GROWTH = 1.1;

const HARD = {
  rustProduction: 1_600,
  tsx: 550,
  typescript: 700,
  script: 1_000,
} as const;

const RATCHET_ONLY = new Set([
  "src-tauri/src/lib.rs",
  "scripts/larm-readiness.ts",
]);

export type SizeRecord = {
  path: string;
  total: number;
  production: number;
};

export type BaselineFile = {
  generatedAt: string;
  files: Record<string, { total: number; production: number }>;
};

export function walk(directory: string, files: string[] = []): string[] {
  for (const entry of readdirSync(directory)) {
    if (entry === "node_modules" || entry === "target" || entry === "dist" || entry === ".git") continue;
    const path = join(directory, entry);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) continue;
    if (stat.isDirectory()) walk(path, files);
    else if (stat.isFile()) files.push(path);
  }
  return files;
}

function posix(path: string): string {
  return relative(ROOT, path).split(sep).join("/");
}

export function productionLines(content: string, path: string): number {
  if (!path.endsWith(".rs")) return content.split("\n").length;
  const matches = [...content.matchAll(/\n#\[cfg\([^\]\n]*\btest\b[^\]\n]*\)\]\s*\nmod tests \{/g)];
  for (const match of matches.reverse()) {
    if (match.index === undefined) continue;
    const openingBrace = match.index + match[0].lastIndexOf("{");
    const closingBrace = matchingRustBrace(content, openingBrace);
    if (closingBrace !== null && content.slice(closingBrace + 1).trim() === "") {
      return content.slice(0, match.index).split("\n").length;
    }
  }
  return content.split("\n").length;
}

function matchingRustBrace(content: string, openingBrace: number): number | null {
  let depth = 0;
  for (let index = openingBrace; index < content.length; index += 1) {
    const character = content[index];
    const next = content[index + 1];
    if (character === "/" && next === "/") {
      index = content.indexOf("\n", index + 2);
      if (index === -1) return null;
      continue;
    }
    if (character === "/" && next === "*") {
      let commentDepth = 1;
      index += 2;
      while (index < content.length && commentDepth > 0) {
        if (content[index] === "/" && content[index + 1] === "*") {
          commentDepth += 1;
          index += 2;
        } else if (content[index] === "*" && content[index + 1] === "/") {
          commentDepth -= 1;
          index += 2;
        } else {
          index += 1;
        }
      }
      index -= 1;
      continue;
    }
    const raw = content.slice(index).match(/^(?:br|r)(#*)"/);
    if (raw) {
      const terminator = `"${raw[1]}`;
      const end = content.indexOf(terminator, index + raw[0].length);
      if (end === -1) return null;
      index = end + terminator.length - 1;
      continue;
    }
    if (character === '"') {
      index = quotedRustLiteralEnd(content, index, '"');
      if (index === -1) return null;
      continue;
    }
    if (character === "'" && rustCharLiteralEnd(content, index) !== null) {
      index = rustCharLiteralEnd(content, index) ?? index;
      continue;
    }
    if (character === "{") depth += 1;
    if (character === "}" && --depth === 0) return index;
  }
  return null;
}

function quotedRustLiteralEnd(content: string, start: number, quote: string): number {
  for (let index = start + 1; index < content.length; index += 1) {
    if (content[index] === "\\") index += 1;
    else if (content[index] === quote) return index;
  }
  return -1;
}

function rustCharLiteralEnd(content: string, start: number): number | null {
  const limit = Math.min(content.length, start + 16);
  for (let index = start + 1; index < limit; index += 1) {
    if (content[index] === "\\") index += 1;
    else if (content[index] === "'") return index;
    else if (/\s/u.test(content[index])) return null;
  }
  return null;
}

export function collectSizes(): SizeRecord[] {
  const roots = [join(ROOT, "src"), join(ROOT, "src-tauri/src"), join(ROOT, "tests"), join(ROOT, "scripts")];
  const records: SizeRecord[] = [];
  for (const root of roots) {
    if (!existsSync(root)) continue;
    for (const path of walk(root)) {
      if (!/\.(rs|ts|tsx|css)$/.test(path)) continue;
      const content = readFileSync(path, "utf8");
      const total = content.split("\n").length;
      records.push({ path: posix(path), total, production: productionLines(content, posix(path)) });
    }
  }
  return records.sort((left, right) => left.path.localeCompare(right.path));
}

function hardLimit(record: SizeRecord): number | undefined {
  if (record.path === "src/App.tsx") return 450;
  if (RATCHET_ONLY.has(record.path)) return undefined;
  if (record.path.endsWith(".rs")) return HARD.rustProduction;
  if (record.path.endsWith(".tsx")) return HARD.tsx;
  if (record.path.startsWith("scripts/") && record.path.endsWith(".ts")) return HARD.script;
  if (record.path.endsWith(".ts")) return HARD.typescript;
  return undefined;
}

export function evaluate(records: SizeRecord[], baseline: BaselineFile): string[] {
  const failures: string[] = [];
  for (const record of records) {
    const previous = baseline.files[record.path];
    const measured = record.path.endsWith(".rs") ? record.production : record.total;
    if (previous) {
      const previousMeasured = record.path.endsWith(".rs") ? previous.production : previous.total;
      const ceiling = Math.ceil(previousMeasured * GROWTH);
      if (measured > ceiling) {
        failures.push(`${record.path}: ${measured} exceeds ratchet ${ceiling} (baseline ${previousMeasured})`);
      }
    }
    const hard = hardLimit(record);
    if (hard !== undefined && measured > hard) {
      const allowed = previous && (record.path.endsWith(".rs") ? previous.production : previous.total) > hard;
      if (!allowed) {
        failures.push(`${record.path}: ${measured} exceeds hard budget ${hard}`);
      }
    }
  }
  return failures;
}

function usage(): never {
  console.error("usage: bun scripts/module-size.ts check|write-baseline");
  process.exit(64);
}

if (import.meta.main) {
  const command = process.argv[2];
  if (command !== "check" && command !== "write-baseline" && command !== undefined) usage();
  const records = collectSizes();
  if (command === "write-baseline") {
    const baseline: BaselineFile = {
      generatedAt: new Date().toISOString(),
      files: Object.fromEntries(records.map((record) => [record.path, { total: record.total, production: record.production }])),
    };
    writeFileSync(BASELINE_PATH, `${JSON.stringify(baseline, null, 2)}\n`);
    console.log(`wrote ${records.length} files to ${posix(BASELINE_PATH)}`);
  } else {
    if (!existsSync(BASELINE_PATH)) {
      console.error(`missing ${posix(BASELINE_PATH)}; run bun scripts/module-size.ts write-baseline`);
      process.exit(2);
    }
    const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8")) as BaselineFile;
    const failures = evaluate(records, baseline);
    if (failures.length > 0) {
      console.error(failures.join("\n"));
      process.exit(1);
    }
    console.log(`module-size ok (${records.length} files)`);
  }
}
