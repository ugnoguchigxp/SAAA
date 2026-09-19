// Regenerates the llang-capability-v2 fixtures from a fixed L-Lang checkout.
//
//   LLANG_ROOT=/path/to/L-Lang bun src-tauri/tests/fixtures/llang-capability-v2/tools/generate.ts
//
// The generator never ships: SAAA only consumes the committed fixture tree. It bundles the
// L-Lang host CLI and its workers into runtime/ (runtime bundle), packages candidate-a and
// candidate-b with the L-Lang v2 packager, writes SAAA-side acceptance cases, and records
// raw host responses under wire/.
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";

const OUT = resolve(import.meta.dir, "..");
const LLANG_ROOT = resolve(process.env.LLANG_ROOT ?? "/Users/y.noguchi/Code/L-Lang");
const EXAMPLE = join(LLANG_ROOT, "examples/jsonc-enabled-user");
const SCHEMAS = join(LLANG_ROOT, "examples/saaa-host");

const { packageLlangCapability } = await import(
  join(LLANG_ROOT, "src/llang-capability-runtime.ts")
);
const { runHostRequest } = await import(join(LLANG_ROOT, "src/capability-host.ts"));
const { HOST_PROTOCOL } = await import(join(LLANG_ROOT, "src/capability-host.ts"));

const digest = (bytes: Uint8Array | string) =>
  createHash("sha256").update(bytes).digest("hex");

const fileHash = async (path: string) => digest(await readFile(path));

async function git(cwd: string, args: string[]) {
  const result = Bun.spawnSync(["git", ...args], { cwd });
  return result.exitCode === 0 ? result.stdout.toString().trim() : null;
}

async function buildRuntime() {
  const root = join(OUT, "runtime");
  await rm(root, { recursive: true, force: true });
  await mkdir(root, { recursive: true });
  const bundled = await Bun.build({
    entrypoints: [
      "capability-host-cli.ts",
      "capability-worker.ts",
      "capability-invoke-worker.ts",
      "llang-capability-worker.ts",
    ].map((name) => join(LLANG_ROOT, "src", name)),
    outdir: root,
    target: "bun",
    naming: "[name].ts",
  });
  if (!bundled.success) throw new Error("runtime bundle failed");
  for (const schema of ["request.schema.json", "response.schema.json"])
    await cp(join(SCHEMAS, schema), join(root, schema));
  const files = [
    "capability-host-cli.ts",
    "capability-worker.ts",
    "capability-invoke-worker.ts",
    "llang-capability-worker.ts",
    "request.schema.json",
    "response.schema.json",
  ];
  const hashes: Record<string, string> = {};
  for (const file of files) hashes[file] = await fileHash(join(root, file));
  const manifest = {
    formatVersion: 1,
    entrypoint: "capability-host-cli.ts",
    requestSchema: "request.schema.json",
    responseSchema: "response.schema.json",
    bunVersion: Bun.version,
    llangVersion: await git(LLANG_ROOT, ["rev-parse", "HEAD"]),
    llangDirty: (await git(LLANG_ROOT, ["status", "--porcelain"])) !== "",
    files: hashes,
  };
  await writeFile(join(root, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

const CANDIDATE_B_SOURCE = `{
  "language": "l-lang",
  "version": 1,
  "id": "enabled-user",
  "profile": "predicate-i32-v1",
  "description": "有効な利用者を判定する。停止フラグは参照しない。",
  "contract": {
    "version": 1,
    "fields": [
      { "name": "enabled", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false },
      { "name": "suspended", "kind": "boolean", "values": [], "nullable": false, "undefinable": false, "optional": false }
    ]
  },
  "body": {
    "kind": "all",
    "conditions": [
      { "kind": "equals", "property": ["enabled"], "value": true }
    ]
  }
}
`;

async function packageCandidates() {
  const metadata = JSON.parse(await readFile(join(EXAMPLE, "metadata.json"), "utf8"));
  const requestPath = join(EXAMPLE, "request.json");
  const request = JSON.parse(await readFile(requestPath, "utf8"));
  const requestRevision = (await import(join(LLANG_ROOT, "src/llang-capability-contracts.ts")))
    .requestRevision(request);
  const contractHash = (await import(join(LLANG_ROOT, "src/prompt-source.ts"))).contentHash(
    request.contract,
  );

  for (const dir of ["candidate-a", "candidate-b"])
    await rm(join(OUT, dir), { recursive: true, force: true });

  const a = await packageLlangCapability(
    join(EXAMPLE, "enabled-user.llang.jsonc"),
    requestPath,
    join(EXAMPLE, "tests.json"),
    metadata,
    join(OUT, "candidate-a"),
  );

  const tmp = join(OUT, "tools", ".tmp");
  await rm(tmp, { recursive: true, force: true });
  await mkdir(tmp, { recursive: true });
  const sourceB = join(tmp, "enabled-user.llang.jsonc");
  const testsB = join(tmp, "tests-b.json");
  await writeFile(sourceB, CANDIDATE_B_SOURCE);
  await writeFile(
    testsB,
    `${JSON.stringify(
      {
        version: 2,
        requestRevision,
        contractHash,
        cases: [
          {
            id: "accept",
            requirementIds: ["enabled", "not-suspended"],
            input: { enabled: true, suspended: false },
            undefinedFields: [],
            expected: { kind: "value", value: true },
          },
          {
            id: "disabled",
            requirementIds: ["enabled"],
            input: { enabled: false, suspended: false },
            undefinedFields: [],
            expected: { kind: "value", value: false },
          },
          {
            id: "suspended",
            requirementIds: ["not-suspended"],
            input: { enabled: true, suspended: true },
            undefinedFields: [],
            expected: { kind: "value", value: true },
          },
        ],
      },
      null,
      2,
    )}\n`,
  );
  const b = await packageLlangCapability(
    sourceB,
    requestPath,
    testsB,
    metadata,
    join(OUT, "candidate-b"),
  );
  await rm(tmp, { recursive: true, force: true });
  return { a, b, requestRevision, contractHash, metadata };
}

async function writeAcceptance(context: {
  requestRevision: string;
  contractHash: string;
  metadata: unknown;
}) {
  const root = join(OUT, "acceptance");
  await rm(root, { recursive: true, force: true });
  await mkdir(root, { recursive: true });
  const table = (id: string, suspendedTrue: boolean) => ({
    version: 1,
    id,
    capabilityId: "enabled-user",
    contract: {
      fields: [
        { name: "enabled", kind: "boolean" },
        { name: "suspended", kind: "boolean" },
      ],
    },
    cases: [
      { id: "ff", input: { enabled: false, suspended: false }, expected: false },
      { id: "ft", input: { enabled: false, suspended: true }, expected: false },
      { id: "tf", input: { enabled: true, suspended: false }, expected: true },
      { id: "tt", input: { enabled: true, suspended: true }, expected: suspendedTrue },
    ],
  });
  const enabledUser = table("enabled-user-truth-table", false);
  const enabledOnly = table("enabled-only-truth-table", true);
  await writeFile(
    join(root, "enabled-user.truth-table.json"),
    `${JSON.stringify(enabledUser, null, 2)}\n`,
  );
  await writeFile(
    join(root, "enabled-only.truth-table.json"),
    `${JSON.stringify(enabledOnly, null, 2)}\n`,
  );
  const entries = [
    {
      id: "enabled-user-truth-table",
      file: "enabled-user.truth-table.json",
      hash: await fileHash(join(root, "enabled-user.truth-table.json")),
    },
    {
      id: "enabled-only-truth-table",
      file: "enabled-only.truth-table.json",
      hash: await fileHash(join(root, "enabled-only.truth-table.json")),
    },
  ];
  await writeFile(
    join(root, "index.json"),
    `${JSON.stringify({ formatVersion: 1, entries }, null, 2)}\n`,
  );
  return entries;
}

async function writeWire(hashes: Record<string, string>) {
  const root = join(OUT, "wire");
  await rm(root, { recursive: true, force: true });
  await mkdir(root, { recursive: true });
  const invoke = (packageHash: string, id: string, input: unknown) => ({
    protocol: HOST_PROTOCOL,
    requestId: id,
    operation: "invoke",
    packageHash,
    input,
    undefinedFields: [],
    timeoutMs: 10000,
  });
  const requests: Record<string, unknown> = {
    "inspect-a.json": {
      protocol: HOST_PROTOCOL,
      requestId: "wire-inspect",
      operation: "inspect",
      packageHash: hashes["candidate-a"],
    },
    "verify-a.json": {
      protocol: HOST_PROTOCOL,
      requestId: "wire-verify-a",
      operation: "verify",
      packageHash: hashes["candidate-a"],
    },
    "verify-b.json": {
      protocol: HOST_PROTOCOL,
      requestId: "wire-verify-b",
      operation: "verify",
      packageHash: hashes["candidate-b"],
    },
    "invoke-true.json": invoke(hashes["candidate-a"], "wire-invoke-true", {
      enabled: true,
      suspended: false,
    }),
    "invoke-false.json": invoke(hashes["candidate-a"], "wire-invoke-false", {
      enabled: true,
      suspended: true,
    }),
    "invoke-invalid-type.json": invoke(hashes["candidate-a"], "wire-invoke-invalid-type", {
      enabled: "true",
      suspended: false,
    }),
  };
  for (const [name, request] of Object.entries(requests)) {
    const manifest =
      name === "candidate-b" || name.includes("-b")
        ? join(OUT, "candidate-b/capability.json")
        : join(OUT, "candidate-a/capability.json");
    const response = await runHostRequest(manifest, request);
    await writeFile(
      join(root, name),
      `${JSON.stringify({ request, response }, null, 2)}\n`,
    );
  }
}

const runtime = await buildRuntime();
const { a, b, requestRevision, contractHash, metadata } = await packageCandidates();
const acceptance = await writeAcceptance({ requestRevision, contractHash, metadata });
await writeWire({ "candidate-a": a.packageHash, "candidate-b": b.packageHash });

await writeFile(
  join(OUT, "README.md"),
  `# llang-capability-v2 fixtures

Fixed inputs for the SAAA generated-capability runtime (M0/M1). Do not edit by hand.

Generated by:

\`\`\`sh
LLANG_ROOT=<L-Lang checkout> bun src-tauri/tests/fixtures/llang-capability-v2/tools/generate.ts
\`\`\`

- L-Lang: \`${runtime.llangVersion}\` (dirty: ${runtime.llangDirty})
- Bun: \`${runtime.bunVersion}\`
- candidate-a packageHash: \`${a.packageHash}\`
- candidate-b packageHash: \`${b.packageHash}\`
- requestRevision: \`${requestRevision}\`
- contractHash: \`${contractHash}\`

Layout:

| Directory | Contents |
| --- | --- |
| \`runtime/\` | Bundled L-Lang host CLI and workers plus request/response schemas and \`manifest.json\` (file list + hashes) |
| \`candidate-a/\` | v2 package: \`enabled && !suspended\` |
| \`candidate-b/\` | v2 package with the same contract: \`enabled\` only |
| \`acceptance/\` | SAAA-side independent acceptance truth tables and \`index.json\` |
| \`wire/\` | Raw \`{ request, response }\` pairs captured from the real host |
`,
);

console.log(
  JSON.stringify(
    {
      llang: runtime.llangVersion,
      bun: runtime.bunVersion,
      candidateA: a.packageHash,
      candidateB: b.packageHash,
      requestRevision,
      contractHash,
      acceptance,
    },
    null,
    2,
  ),
);
