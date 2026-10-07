import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ROOT } from "../scripts/verify";
import { acquireVerificationLock } from "../scripts/verification-lock";
import { SELECTED_ALLOWLIST } from "../scripts/verify-affected-inputs";

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

test("a package added during initial plan construction is checked before OK", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-plan-inputs-"));
  const logDirectory = mkdtempSync(join(tmpdir(), "saaa-verify-plan-inputs-log-"));
  const log = join(logDirectory, "ran.txt");
  try {
    initRepository(directory);
    const runner = join(directory, "runner.ts");
    writeFileSync(
      runner,
      `import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { runVerification } from ${JSON.stringify(join(ROOT, "scripts/verify.ts"))};
import { verificationPlan } from ${JSON.stringify(join(ROOT, "scripts/verify-plan.ts"))};
const root = ${JSON.stringify(directory)};
const logPath = ${JSON.stringify(log)};
let calls = 0;
const plan = () => {
  const discovered = verificationPlan(["typecheck", "--scope", "rust"], root);
  // Model a concurrent edit after package discovery and before the plan returns.
  if (++calls === 1) {
    const added = join(root, "crates", "added");
    mkdirSync(added, { recursive: true });
    writeFileSync(join(added, "Cargo.toml"), '[package]\\nname="added"\\nversion="0.1.0"\\n');
  }
  // Successful fixture checkers isolate orchestration from actual Rust builds.
  return discovered.map((step) => ({
    name: step.name,
    command: [process.execPath, "-e", 'require("node:fs").appendFileSync(' + JSON.stringify(logPath) + ',' + JSON.stringify(step.name + "\\n") + ')'],
  }));
};
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
      log: "typecheck (src-tauri)\ntypecheck (crates/added)\ntypecheck (src-tauri)\n",
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(logDirectory, { recursive: true, force: true });
  }
});

async function selectedFixture(
  scenario:
    | "wait"
    | "freeze"
    | "expand"
    | "unwatched"
    | "scan-failure"
    | "wide-unscannable"
    | "twice",
) {
  const directory = mkdtempSync(join(tmpdir(), "saaa-selected-window-"));
  const outputs = mkdtempSync(join(tmpdir(), "saaa-selected-output-"));
  let held: Awaited<ReturnType<typeof acquireVerificationLock>> | undefined;
  let child: ReturnType<typeof Bun.spawn> | undefined;
  try {
    initRepository(directory);
    for (const path of SELECTED_ALLOWLIST) {
      mkdirSync(join(directory, path, ".."), { recursive: true });
      writeFileSync(join(directory, path), "baseline");
    }
    const committed = Bun.spawnSync(["git", "add", "."], { cwd: directory });
    expect(committed.exitCode).toBe(0);
    expect(
      Bun.spawnSync(
        [
          "git",
          "-c",
          "user.name=Fixture",
          "-c",
          "user.email=fixture@example.invalid",
          "commit",
          "-qm",
          "inputs",
        ],
        { cwd: directory },
      ).exitCode,
    ).toBe(0);
    const avatar = join(directory, SELECTED_ALLOWLIST[0]!);
    writeFileSync(avatar, "changed");
    if (["wait", "freeze", "wide-unscannable"].includes(scenario))
      writeFileSync(join(directory, "outside.txt"), "wide");
    if (scenario === "wait")
      held = await acquireVerificationLock(directory, new AbortController().signal);
    const runner = join(outputs, "runner.ts");
    const report = join(outputs, "report.json");
    const ready = join(outputs, "ready");
    writeFileSync(
      runner,
      `
import { existsSync, writeFileSync, rmSync, symlinkSync } from "node:fs";
import { join } from "node:path";
import { runVerification, selectedPlan } from ${JSON.stringify(join(ROOT, "scripts/verify-run.ts"))};
const root = ${JSON.stringify(directory)};
const scenario = ${JSON.stringify(scenario)};
const context = { requestedMode: "selected", mode: "selected", level: "normal", requireWatch: scenario !== "unwatched" };
if (scenario === "wide-unscannable") symlinkSync(${JSON.stringify(runner)}, join(root, "escape"));
let calls = 0;
const running = runVerification(() => {
  const plan = selectedPlan(context, "normal", root);
  ++calls;
  if (calls === 1 && scenario === "freeze") rmSync(join(root, "outside.txt"));
  if (calls === 1 && scenario === "expand") writeFileSync(join(root, "outside.txt"), "expanded");
  if (scenario === "twice") writeFileSync(join(root, ${JSON.stringify(SELECTED_ALLOWLIST[0])}), "change-" + calls);
  if (calls === 1 && scenario === "scan-failure") symlinkSync(${JSON.stringify(runner)}, join(root, "escape"));
  return plan.map(step => ({ ...step, command: [process.execPath, "-e", "process.exit(0)"] }));
}, root, context);
writeFileSync(${JSON.stringify(ready)}, "waiting");
process.exitCode = await running;
`,
    );
    child = Bun.spawn([process.execPath, runner], {
      cwd: directory,
      stdout: "pipe",
      stderr: "pipe",
      env: {
        ...process.env,
        SAAA_VERIFY_REPORT: report,
        SAAA_VERIFY_LOCK_PATH: "",
        SAAA_VERIFY_LOCK_TOKEN: "",
      },
    });
    if (scenario === "wait") {
      const deadline = Date.now() + 5_000;
      while (!existsSync(ready)) {
        if (Date.now() > deadline) throw new Error("waiter not ready");
        await Bun.sleep(10);
      }
      rmSync(join(directory, "outside.txt"));
      held!.release();
      held = undefined;
    }
    const [code, stdout, stderr] = await Promise.all([
      child.exited,
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
    ]);
    const body = JSON.parse(readFileSync(report, "utf8")) as {
      requestedMode: string;
      mode: string;
      status: number;
      level: string;
      provesInputUnchanged: boolean;
      watcherDetail: { scanned: boolean; scanMatched: boolean; failed: boolean };
      attemptLog: Array<{
        mode: string;
        fallback: string | null;
        executedSteps: string[];
        watcher: { scanned: boolean; scanMatched: boolean };
      }>;
    };
    return { code, stdout, stderr, body };
  } finally {
    held?.release();
    if (child && child.exitCode === null) {
      child.kill();
      await child.exited;
    }
    rmSync(directory, { recursive: true, force: true });
    rmSync(outputs, { recursive: true, force: true });
  }
}

test("lock wait reselects allowlisted inputs only after attaching the watcher", async () => {
  const { code, stdout, stderr, body } = await selectedFixture("wait");
  expect({ code, stdout, stderr }).toEqual({ code: 0, stdout: "OK\n", stderr: "" });
  expect(body.mode).toBe("selected");
  expect(body.watcherDetail.scanned).toBe(true);
  expect(body.watcherDetail.scanMatched).toBe(true);
});

test("fallback remains full normal on retry after inputs return to the allowlist", async () => {
  const { code, stdout, body } = await selectedFixture("freeze");
  expect({ code, stdout }).toEqual({ code: 0, stdout: "OK\n" });
  expect(body.requestedMode).toBe("selected");
  expect(body.level).toBe("normal");
  expect(body.attemptLog.map((attempt) => attempt.mode)).toEqual([
    "selected-fallback",
    "selected-fallback",
  ]);
  for (const attempt of body.attemptLog) {
    expect(attempt.executedSteps).toContain("format (src-tauri)");
    expect(attempt.executedSteps.some((step) => /build|test/.test(step))).toBe(false);
  }
});

test("an expanding narrow plan records each attempt's own mode and watch result", async () => {
  const { code, stdout, body } = await selectedFixture("expand");
  expect({ code, stdout }).toEqual({ code: 0, stdout: "OK\n" });
  expect(body.mode).toBe("selected-fallback");
  expect(body.attemptLog.map((attempt) => attempt.mode)).toEqual(["selected", "selected-fallback"]);
  expect(body.attemptLog[0]!.watcher.scanMatched).toBe(false);
  expect(body.attemptLog[0]!.executedSteps).not.toContain("format (src-tauri)");
  expect(body.attemptLog[1]!.executedSteps).toContain("format (src-tauri)");
});

for (const scenario of ["unwatched", "scan-failure", "twice"] as const) {
  test(`${scenario} narrow inputs fail with 2 and no OK`, async () => {
    const { code, stdout, body } = await selectedFixture(scenario);
    expect(code).toBe(2);
    expect(stdout).not.toContain("OK");
    expect(body.mode).toBe("selected");
    expect(body.provesInputUnchanged).toBe(false);
  });
}

test("full fallback relies on its fingerprint when the narrow scan is unavailable", async () => {
  const { code, stdout, body } = await selectedFixture("wide-unscannable");
  expect({ code, stdout }).toEqual({ code: 0, stdout: "OK\n" });
  expect(body.mode).toBe("selected-fallback");
  expect(body.watcherDetail.failed).toBe(true);
});
