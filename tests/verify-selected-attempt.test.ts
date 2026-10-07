import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { acquireVerificationLock } from "../scripts/verification-lock";
import {
  decideSelectedExecution,
  runVerification,
  type VerificationContext,
} from "../scripts/verify-run";

const allow = "src/features/chat/avatar/LightAvatarBackground.tsx";

function git(directory: string, ...args: string[]) {
  const result = spawnSync("git", args, { cwd: directory, stdout: "pipe", stderr: "pipe" });
  if (result.status !== 0) throw new Error(new TextDecoder().decode(result.stderr));
}

function repository(): string {
  const directory = mkdtempSync(join(tmpdir(), "saaa-selected-"));
  git(directory, "init", "-q");
  git(
    directory,
    "-c",
    "user.email=test@example.com",
    "-c",
    "user.name=test",
    "commit",
    "-q",
    "--allow-empty",
    "-m",
    "init",
  );
  return directory;
}

const ok = (name: string) => ({ name, command: [process.execPath, "-e", "process.exit(0)"] });

test("selected fallback stays wide even after the inputs return to the allowlist", () => {
  const first = decideSelectedExecution({
    level: "normal",
    holdFallback: false,
    paths: ["outside.txt"],
    planFallback: null,
  });
  const again = decideSelectedExecution({
    level: "normal",
    holdFallback: first.holdFallback,
    paths: [allow],
    planFallback: null,
  });
  expect(again.mode).toBe("selected-fallback");
  expect(again.narrowSteps).toBe(false);
  expect(again.watchRequired).toBe(false);
});

test("a narrow attempt that grows is replanned as a fallback", async () => {
  const directory = repository();
  const report = join(tmpdir(), `saaa-selected-report-${process.pid}.json`);
  mkdirSync(join(directory, "src/features/chat/avatar"), { recursive: true });
  writeFileSync(join(directory, allow), "avatar");
  const previous = process.env.SAAA_VERIFY_REPORT;
  process.env.SAAA_VERIFY_REPORT = report;
  const context: VerificationContext = {
    requestedMode: "selected",
    mode: "selected",
    level: "normal",
    requireWatch: true,
  };
  try {
    let calls = 0;
    const code = await runVerification(
      () => {
        calls += 1;
        if (context.frozenFallback) {
          context.mode = "selected-fallback";
          context.fallback = "selected fallback stays wide for this invocation";
          return [ok("wide")];
        }
        const paths = calls === 1 ? [allow] : [allow, "outside.txt"];
        const decision = decideSelectedExecution({
          level: "normal",
          holdFallback: false,
          paths,
          planFallback: null,
        });
        context.frozenFallback = decision.holdFallback;
        context.mode = decision.mode;
        context.fallback = decision.fallback;
        if (calls === 1) writeFileSync(join(directory, "outside.txt"), "later");
        return [ok(decision.mode)];
      },
      directory,
      context,
    );
    expect(code).toBe(0);
    const body = JSON.parse(readFileSync(report, "utf8")) as {
      requestedMode: string;
      mode: string;
      attemptLog: Array<{ mode: string; fallback: string | null; executedSteps: string[] }>;
    };
    expect(body.requestedMode).toBe("selected");
    expect(body.mode).toBe("selected-fallback");
    expect(body.attemptLog[0]?.mode).toBe("selected");
    expect(body.attemptLog[1]?.mode).toBe("selected-fallback");
    expect(body.attemptLog[0]?.executedSteps).toEqual(["selected"]);
    expect(body.attemptLog[1]?.executedSteps).toEqual(["selected-fallback"]);
  } finally {
    if (previous === undefined) delete process.env.SAAA_VERIFY_REPORT;
    else process.env.SAAA_VERIFY_REPORT = previous;
    rmSync(directory, { recursive: true, force: true });
    rmSync(report, { force: true });
  }
});

test("a lock wait that moves onto the allowlist does not run narrow without a watcher", async () => {
  const directory = repository();
  writeFileSync(join(directory, "outside.txt"), "wide");
  const lock = await acquireVerificationLock(directory, new AbortController().signal);
  const report = join(tmpdir(), `saaa-selected-lock-${process.pid}.json`);
  const runner = join(directory, "runner.ts");
  writeFileSync(
    runner,
    `import { existsSync } from "node:fs";
import { join } from "node:path";
import { decideSelectedExecution, runVerification } from ${JSON.stringify(join(import.meta.dir, "../scripts/verify-run.ts"))};
const root = ${JSON.stringify(directory)};
const allow = ${JSON.stringify(allow)};
const context = { requestedMode: "selected", mode: "selected", level: "normal", requireWatch: true };
process.env.SAAA_VERIFY_REPORT = ${JSON.stringify(report)};
process.exitCode = await runVerification(() => {
  const paths = [existsSync(join(root, "outside.txt")) ? "outside.txt" : "", existsSync(join(root, allow)) ? allow : ""].filter(Boolean);
  const decision = decideSelectedExecution({ level: "normal", holdFallback: false, paths, planFallback: null });
  if (decision.narrowSteps && context.watcherAttached !== true) {
    context.refuseUnwatched = true;
    context.mode = "selected";
    return [];
  }
  context.mode = decision.mode;
  context.fallback = decision.fallback;
  return [{ name: decision.mode, command: [process.execPath, "-e", "process.exit(0)"] }];
}, root, context);
`,
  );
  const child = Bun.spawn([process.execPath, runner], {
    cwd: directory,
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env, SAAA_VERIFY_LOCK_PATH: "", SAAA_VERIFY_LOCK_TOKEN: "" },
  });
  try {
    await new Promise((resolve) => setTimeout(resolve, 250));
    rmSync(join(directory, "outside.txt"));
    mkdirSync(join(directory, "src/features/chat/avatar"), { recursive: true });
    writeFileSync(join(directory, allow), "avatar");
    lock.release();
    const code = await child.exited;
    const stderr = await new Response(child.stderr).text();
    expect({ code, stderr }).toEqual({ code: 0, stderr: "" });
    const body = JSON.parse(readFileSync(report, "utf8")) as {
      mode: string;
      watcher: string;
      watcherDetail: { scanned: boolean; scanMatched: boolean };
    };
    expect(body.mode).toBe("selected");
    expect(body.watcher).toBe("attached");
    expect(body.watcherDetail.scanned).toBe(true);
    expect(body.watcherDetail.scanMatched).toBe(true);
  } finally {
    lock.release();
    child.kill();
    rmSync(directory, { recursive: true, force: true });
    rmSync(report, { force: true });
  }
});
