import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ROOT } from "../scripts/verify";
import { renderedDiagnostics } from "../scripts/clippy-diagnostics";
import { rustPackages, verificationPlan } from "../scripts/verify-plan";

async function fixture(commands: string[], env: Record<string, string> = {}) {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-test-"));
  initRepository(directory);
  const runner = join(directory, "runner.ts");
  writeFileSync(
    runner,
    `import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
const steps = ${JSON.stringify(commands)}.map((source, i) => ({
  name: "step " + i, command: [process.execPath, "-e", source]
}));
process.exitCode = await runVerification(steps, ${JSON.stringify(directory)});`,
  );
  const child = Bun.spawn([process.execPath, runner], {
    cwd: directory,
    env: { ...process.env, ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { directory, stdout, stderr, code };
}

function initRepository(directory: string) {
  const git = (...args: string[]) => {
    const result = Bun.spawnSync(
      ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", ...args],
      { cwd: directory, stdout: "pipe", stderr: "pipe" },
    );
    if (result.exitCode !== 0) throw new Error(new TextDecoder().decode(result.stderr));
  };
  git("init", "--quiet");
  git("commit", "--quiet", "--allow-empty", "-m", "fixture");
}

test("an unmeasured tree is not reported as OK", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-unmeasured-"));
  const runner = join(directory, "runner.ts");
  writeFileSync(
    runner,
    `import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
process.exitCode = await runVerification([{ name: "step", command: [process.execPath, "-e", ""] }], ${JSON.stringify(directory)});`,
  );
  const child = Bun.spawn([process.execPath, runner], {
    cwd: directory,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  try {
    expect(code).toBe(2);
    expect(stdout).not.toContain("OK");
    expect(stderr).toContain("UNMEASURED");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a missing report directory fails before OK", async () => {
  const result = await fixture(['console.log("ready")'], {
    SAAA_VERIFY_REPORT: join(tmpdir(), "saaa-verify-missing-parent", "report.json"),
  });
  try {
    expect(result.code).not.toBe(0);
    expect(result.stdout).not.toContain("OK");
    expect(existsSync(join(tmpdir(), "saaa-verify-missing-parent", "report.json"))).toBe(false);
  } finally {
    rmSync(result.directory, { recursive: true, force: true });
  }
});

test("a retry rebuilds the plan and includes a package added during the run", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-rebuild-"));
  const logDirectory = mkdtempSync(join(tmpdir(), "saaa-verify-rebuild-log-"));
  const log = join(logDirectory, "ran.txt");
  try {
    initRepository(directory);
    mkdirSync(join(directory, "crates", "old"), { recursive: true });
    const added = join(directory, "crates", "added");
    const marker = join(added, "marker");
    const runner = join(directory, "runner.ts");
    writeFileSync(
      runner,
      `import { readdirSync } from "node:fs";
import { join } from "node:path";
import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
const root = ${JSON.stringify(directory)};
const logPath = ${JSON.stringify(log)};
const addedPath = ${JSON.stringify(added)};
const markerPath = ${JSON.stringify(marker)};
const plan = () => readdirSync(join(root, "crates")).sort().map((name) => ({
  name,
  command: [process.execPath, "-e",
    'const fs=require("node:fs"); fs.appendFileSync(' + JSON.stringify(logPath) + ',' + JSON.stringify(name + "\\n") + '); if (' + JSON.stringify(name) + '==="old" && !fs.existsSync(' + JSON.stringify(addedPath) + ')) { fs.mkdirSync(' + JSON.stringify(addedPath) + '); fs.writeFileSync(' + JSON.stringify(markerPath) + ',"1"); }'
  ],
}));
process.exitCode = await runVerification(plan, root);
`,
    );
    const child = Bun.spawn([process.execPath, runner], {
      cwd: directory,
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);
    expect({ code, stdout, stderr, log: readFileSync(log, "utf8") }).toEqual({
      code: 0,
      stdout: "OK\n",
      stderr: "",
      log: "old\nadded\nold\n",
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(logDirectory, { recursive: true, force: true });
  }
});

test("the first plan is built after the lock includes a package added while waiting", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-locked-plan-"));
  const logDirectory = mkdtempSync(join(tmpdir(), "saaa-verify-locked-plan-log-"));
  const log = join(logDirectory, "ran.txt");
  const held = join(directory, "held");
  const release = join(directory, "held.release");
  try {
    initRepository(directory);
    mkdirSync(join(directory, "crates", "early"), { recursive: true });
    const holder = join(directory, "holder.ts");
    const holdSource = `const fs=require("node:fs"); fs.writeFileSync(${JSON.stringify(held)}, "held"); while (!fs.existsSync(${JSON.stringify(release)})) await Bun.sleep(10);`;
    writeFileSync(
      holder,
      `import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
process.exitCode = await runVerification([{ name: "hold", command: [process.execPath, "-e", ${JSON.stringify(holdSource)}] }], ${JSON.stringify(directory)});`,
    );
    const holderChild = Bun.spawn([process.execPath, holder], {
      cwd: directory,
      stdout: "pipe",
      stderr: "pipe",
    });
    const deadline = Date.now() + 5_000;
    while (!existsSync(held)) {
      if (Date.now() > deadline) throw new Error("holder did not acquire the lock");
      await Bun.sleep(10);
    }
    const waiter = join(directory, "waiter.ts");
    writeFileSync(
      waiter,
      `import { readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
writeFileSync(${JSON.stringify(join(directory, "waiter.ready"))}, "ready");
const root = ${JSON.stringify(directory)};
const logPath = ${JSON.stringify(log)};
const plan = () => readdirSync(join(root, "crates")).sort().map((name) => ({
  name,
  command: [process.execPath, "-e", 'require("node:fs").appendFileSync(' + JSON.stringify(logPath) + ',' + JSON.stringify(name + "\\n") + ')'],
}));
process.exitCode = await runVerification(plan, root);
`,
    );
    const waiterChild = Bun.spawn([process.execPath, waiter], {
      cwd: directory,
      stdout: "pipe",
      stderr: "pipe",
    });
    const ready = join(directory, "waiter.ready");
    const readyDeadline = Date.now() + 5_000;
    while (!existsSync(ready)) {
      if (Date.now() > readyDeadline) throw new Error("waiter did not start");
      await Bun.sleep(10);
    }
    await Bun.sleep(250);
    mkdirSync(join(directory, "crates", "late"));
    writeFileSync(release, "go");
    const [stdout, stderr, code] = await Promise.all([
      new Response(waiterChild.stdout).text(),
      new Response(waiterChild.stderr).text(),
      waiterChild.exited,
    ]);
    holderChild.kill();
    await holderChild.exited;
    expect({ code, stdout, stderr, log: readFileSync(log, "utf8") }).toEqual({
      code: 0,
      stdout: "OK\n",
      stderr: "",
      log: "early\nlate\n",
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(logDirectory, { recursive: true, force: true });
  }
});

test("successful verification emits only one OK, even with noisy tools", async () => {
  const result = await fixture([
    'console.log("compiler progress"); console.error("tool banner")',
    'console.log("test summary")',
  ]);
  try {
    expect(result.code).toBe(0);
    expect(result.stdout).toBe("OK\n");
    expect(result.stderr).toBe("");
  } finally {
    rmSync(result.directory, { recursive: true, force: true });
  }
});

test("failure preserves large stdout and stderr, exit code, and stops subsequent tools", async () => {
  const large = "diagnostic details\n".repeat(70_000);
  const result = await fixture([
    'console.log("previous successful output")',
    'console.log("diagnostic details\\n".repeat(70_000)); console.error("source.ts:42: detailed error"); process.exit(7)',
    'require("node:fs").writeFileSync("must-not-run", "bad")',
  ]);
  try {
    expect(result.code).toBe(7);
    expect(result.stdout).toBe("");
    expect(result.stderr).toContain(large);
    expect(result.stderr).toContain("source.ts:42: detailed error");
    expect(result.stderr).toContain("FAIL step 1");
    expect(result.stderr).not.toContain("previous successful output");
    expect(existsSync(join(result.directory, "must-not-run"))).toBe(false);
  } finally {
    rmSync(result.directory, { recursive: true, force: true });
  }
});

test("invalid selections fail instead of reporting success for checks that never ran", () => {
  for (const args of [
    ["unknown"],
    ["lint", "--write"],
    ["test", "--scope", "rust", "--", "voice::"],
    ["lint", "--scope", "typescript", "--package", "src-tauri"],
    ["ipc", "--scope", "typescript"],
    ["test", "--package", "missing"],
  ]) {
    expect(() => verificationPlan(args, ROOT)).toThrow();
  }
});

test("Rust verification includes all independent packages and supports focused tests", () => {
  const packages = rustPackages(ROOT);
  expect(packages).toContain("crates/terminal-agent-runtime");
  const plan = verificationPlan(["test", "--scope", "rust"], ROOT);
  expect(plan.map((step) => step.command[3])).toEqual(packages.map((path) => `${path}/Cargo.toml`));
  const focused = verificationPlan(
    ["test", "--package", "src-tauri", "--", "--lib", "voice::"],
    ROOT,
  );
  expect(focused).toHaveLength(1);
  expect(focused[0].command.slice(-3)).toEqual(["--locked", "--lib", "voice::"]);
});

test("normal excludes builds; advance adds builds and tests; full adds E2E", () => {
  const normal = verificationPlan([], ROOT);
  const advance = verificationPlan(["advance"], ROOT);
  const full = verificationPlan(["full"], ROOT);
  const runsTests = (step: { command: string[] }) => step.command[1] === "test";
  const runsBuild = (step: { command: string[] }) =>
    step.command[1] === "build" ||
    (step.command.includes("vite") && step.command.includes("build"));
  expect(normal.some(runsBuild)).toBe(false);
  expect(advance.some(runsBuild)).toBe(true);
  expect(verificationPlan(["build"], ROOT).some(runsBuild)).toBe(true);
  expect(verificationPlan(["build", "--scope", "rust"], ROOT).some(runsBuild)).toBe(true);
  expect(normal.some(runsTests)).toBe(false);
  expect(normal.some((step) => step.command.includes("scripts/frontend-tests.ts"))).toBe(false);
  expect(advance.slice(0, normal.length)).toEqual(normal);
  expect(advance.some(runsTests)).toBe(true);
  expect(advance.some((step) => step.command.includes("scripts/frontend-tests.ts"))).toBe(true);
  expect(advance.some((step) => step.command.includes("scripts/desktop-smoke.ts"))).toBe(false);
  expect(full.slice(0, advance.length)).toEqual(advance);
  expect(full.some((step) => step.command.includes("conversation_queue_e2e"))).toBe(true);
  expect(full.some((step) => step.command.includes("scripts/desktop-smoke.ts"))).toBe(true);
  expect(full.some((step) => step.command.includes("scripts/feature-lab-smoke.ts"))).toBe(true);
  expect(advance.some((step) => step.command.includes("scripts/feature-lab-smoke.ts"))).toBe(false);
  expect(verificationPlan(["lab-smoke"], ROOT).map((step) => step.command.join(" "))).toEqual([
    `cargo build --manifest-path services/feature-lab/Cargo.toml --locked --bin saaa-feature-lab`,
    `${process.execPath} scripts/feature-lab-smoke.ts`,
  ]);
});

test("standard package commands have no bypass around verify", () => {
  const { scripts } = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8"));
  for (const name of ["lint", "typecheck", "build", "test", "format"]) {
    expect(scripts[name]).toStartWith(`bun scripts/verify.ts ${name}`);
    expect(scripts[`${name}:rust`]).toStartWith(`bun scripts/verify.ts ${name} --scope rust`);
  }
  expect(scripts.verify).toBe("bun scripts/verify.ts");
  expect(scripts["build:desktop"]).toBe("bun scripts/verify.ts desktop-build");
});

test("Clippy failure diagnostics retain source locations and compiler explanations", () => {
  const rendered =
    "error[E0425]: missing value\n --> src/example.rs:42:7\n  help: use an existing value\n";
  const output = [
    JSON.stringify({ reason: "compiler-artifact", filenames: ["target/example"] }),
    JSON.stringify({ reason: "compiler-message", message: { rendered } }),
    JSON.stringify({ reason: "build-finished", success: false }),
  ].join("\n");
  expect(renderedDiagnostics(output)).toBe(rendered);
});
