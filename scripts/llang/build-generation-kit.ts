// Builds the self-contained L-Lang generation kit consumed by SAAA (plan 12.1, C01).
//
//   bun scripts/llang/build-generation-kit.ts --llang-root <checkout> --out <new-directory> [--selftest]
//
// The kit bundles `src/llang-cli.ts` (and everything it imports) with Bun so that Package/Inspect
// run without the L-Lang checkout, and records an entrypoint, file list, hashes, the L-Lang
// HEAD/dirty state and the Bun version in `manifest.json`. The digest is the SHA-256 of the
// sorted `name\0hash\n` records, matching the runtime-bundle digest convention. The configured
// `expectedKitDigest` is compared against this value; a freshly computed digest is never adopted.
import { createHash } from "node:crypto";
import { cp, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const args = new Map<string, string | boolean>();
for (let index = 2; index < process.argv.length; index += 1) {
  const token = process.argv[index];
  if (!token.startsWith("--")) continue;
  const next = process.argv[index + 1];
  if (next && !next.startsWith("--")) {
    args.set(token.slice(2), next);
    index += 1;
  } else {
    args.set(token.slice(2), true);
  }
}

const LLANG_ROOT = resolve(
  (args.get("llang-root") as string | undefined) ??
    process.env.LLANG_ROOT ??
    "/Users/y.noguchi/Code/L-Lang",
);
const OUT = resolve(
  (args.get("out") as string | undefined) ??
    process.env.SAAA_GENERATION_KIT_OUT ??
    join(process.cwd(), "dist", "llang-generation-kit"),
);
const SELFTEST = args.has("selftest");

const ENTRYPOINT_BUNDLE = "llang-cli.js";
const MANIFEST_FILE = "manifest.json";
const ENTRY_SOURCE = ".entry.ts";

// A thin fixed dispatch that calls the same L-Lang library functions the fixed `llang package` /
// `llang inspect` commands call. Bundling only this graph keeps the kit self-contained without the
// unrelated develop/effects/codex command surface (and avoids a Bun bundler failure in that
// surface). The argument shape matches `docs/LLANG_CLI_REFERENCE.md`.
function entrySource(llangRoot: string): string {
  const capability = JSON.stringify(join(llangRoot, "src/llang-capability.ts"));
  const inspection = JSON.stringify(join(llangRoot, "src/llang-capability-inspection.ts"));
  return `import { packageLlangCapability } from ${capability};
import { inspectLlangCapability } from ${inspection};
import { readFile } from "node:fs/promises";

const [command, ...rest] = process.argv.slice(2);
const option = (name: string) => {
  const index = rest.indexOf(name);
  return index >= 0 ? rest[index + 1] : undefined;
};
const required = (name: string) => {
  const value = option(name);
  if (!value) throw new Error("missing " + name);
  return value;
};

if (command === "--help" || command === "help" || command === undefined) {
  console.log("usage: llang-cli.js package <source.llang.jsonc> --request <request.json> --suite <tests.json> --metadata <metadata.json> --out-dir <new-directory> | llang-cli.js inspect <capability.json> [--out-dir <new-directory>]");
  process.exit(0);
}
if (command === "package") {
  const source = rest[0];
  const metadataPath = required("--metadata");
  const metadata = JSON.parse(await readFile(metadataPath, "utf8"));
  const result = await packageLlangCapability(
    source,
    required("--request"),
    required("--suite"),
    metadata,
    required("--out-dir"),
  );
  console.log(JSON.stringify(result));
} else if (command === "inspect") {
  const capability = rest[0];
  const result = await inspectLlangCapability(capability, option("--out-dir"));
  console.log(JSON.stringify(result));
} else {
  console.error("unsupported kit command");
  process.exit(2);
}
`;
}

const digest = (bytes: Uint8Array | string) => createHash("sha256").update(bytes).digest("hex");

async function git(cwd: string, gitArgs: string[]): Promise<string | null> {
  const result = Bun.spawnSync(["git", ...gitArgs], { cwd });
  return result.exitCode === 0 ? result.stdout.toString().trim() : null;
}

async function listFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  const walk = async (directory: string, prefix: string) => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        await walk(join(directory, entry.name), relative);
      } else if (entry.isFile()) {
        files.push(relative);
      }
    }
  };
  await walk(root, "");
  return files.sort();
}

function kitDigest(files: Record<string, string>): string {
  const text = Object.keys(files)
    .sort()
    .map((name) => `${name}\0${files[name]}\n`)
    .join("");
  return digest(text);
}

async function build(): Promise<Record<string, unknown>> {
  const entry = join(OUT, ENTRY_SOURCE);
  await rm(OUT, { recursive: true, force: true });
  await mkdir(OUT, { recursive: true });
  await writeFile(entry, entrySource(LLANG_ROOT));
  const bundled = await Bun.build({
    entrypoints: [entry],
    outdir: OUT,
    target: "bun",
    naming: ENTRYPOINT_BUNDLE,
  });
  await rm(entry, { force: true });
  if (!bundled.success) throw new Error("generation kit bundle failed");
  await writeFile(
    join(OUT, "package.json"),
    `${JSON.stringify({ name: "saaa-llang-generation-kit", private: true, type: "module" }, null, 2)}\n`,
  );
  const files: Record<string, string> = {};
  for (const name of await listFiles(OUT)) {
    if (name === MANIFEST_FILE) continue;
    files[name] = digest(await readFile(join(OUT, name)));
  }
  const manifest = {
    formatVersion: 1,
    entrypoint: ENTRYPOINT_BUNDLE,
    commands: { package: "package", inspect: "inspect" },
    bunVersion: Bun.version,
    llangVersion: await git(LLANG_ROOT, ["rev-parse", "HEAD"]),
    llangDirty: (await git(LLANG_ROOT, ["status", "--porcelain"])) !== "",
    files,
    digest: kitDigest(files),
  };
  await writeFile(join(OUT, MANIFEST_FILE), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

async function selftest(manifest: Record<string, unknown>): Promise<void> {
  const root = OUT;
  const entrypoint = join(root, manifest.entrypoint as string);
  const help = Bun.spawnSync([process.execPath, entrypoint, "--help"], {
    cwd: root,
  });
  if (help.exitCode !== 0) throw new Error(`kit --help failed: ${help.stderr.toString()}`);

  // Move the kit to a directory whose path contains spaces and prove package/inspect still work
  // without the L-Lang checkout on the module path.
  const parent = await mkdtempWithSpaces();
  const moved = join(parent, "kit with spaces");
  await cp(root, moved, { recursive: true });
  const example = join(LLANG_ROOT, "examples/jsonc-enabled-user");
  const outDir = join(parent, "package output");
  await mkdir(outDir, { recursive: true });
  const packageDir = join(outDir, "candidate");
  const pkg = Bun.spawnSync(
    [
      process.execPath,
      join(moved, manifest.entrypoint as string),
      "package",
      join(example, "enabled-user.llang.jsonc"),
      "--request",
      join(example, "request.json"),
      "--suite",
      join(example, "tests.json"),
      "--metadata",
      join(example, "metadata.json"),
      "--out-dir",
      packageDir,
    ],
    { cwd: parent },
  );
  if (pkg.exitCode !== 0) throw new Error(`kit package failed: ${pkg.stderr.toString()}`);
  const inspect = Bun.spawnSync(
    [
      process.execPath,
      join(moved, manifest.entrypoint as string),
      "inspect",
      join(packageDir, "capability.json"),
      "--json",
    ],
    { cwd: parent },
  );
  if (inspect.exitCode !== 0) throw new Error(`kit inspect failed: ${inspect.stderr.toString()}`);
  const report = JSON.parse(inspect.stdout.toString()) as {
    format?: string;
    typescript?: { projectionHash?: string };
    inspection?: { semanticEquivalence?: string };
  };
  if (report.format !== "llang-capability-inspection")
    throw new Error("inspect returned an unexpected format");
  if (report.inspection?.semanticEquivalence !== "not-checked")
    throw new Error("inspect did not keep semanticEquivalence=not-checked");

  // A tampered file must change the recomputed digest.
  await writeFile(join(moved, "package.json"), '{"tampered":true}\n');
  const files: Record<string, string> = {};
  for (const name of await listFiles(moved)) {
    if (name === MANIFEST_FILE) continue;
    files[name] = digest(await readFile(join(moved, name)));
  }
  if (kitDigest(files) === manifest.digest)
    throw new Error("tampering did not change the kit digest");
  await rm(parent, { recursive: true, force: true });
  console.log("selftest: OK (moved path with spaces, package/inspect, tamper detection)");
}

async function mkdtempWithSpaces(): Promise<string> {
  const base = join(process.env.TMPDIR ?? "/tmp", `saaa kit selftest ${Date.now()}`);
  await mkdir(base, { recursive: true });
  return base;
}

const manifest = await build();
console.log(
  JSON.stringify(
    {
      out: OUT,
      entrypoint: manifest.entrypoint,
      digest: manifest.digest,
      llangVersion: manifest.llangVersion,
      llangDirty: manifest.llangDirty,
      bunVersion: manifest.bunVersion,
      files: Object.keys(manifest.files as Record<string, string>).length,
    },
    null,
    2,
  ),
);
if (SELFTEST) await selftest(manifest);
