import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  runDesktopSmoke,
  sanitizeSmokeLog,
  type SmokeOptions,
} from "../scripts/desktop-smoke-process";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});
function options(script: string): SmokeOptions {
  const directory = mkdtempSync(join(tmpdir(), "saaa-smoke-test-"));
  directories.push(directory);
  return {
    root: directory,
    reportDir: join(directory, "report"),
    build: [process.execPath, "-e", "console.log('build fixture')"],
    executable: [process.execPath, "-e", script],
    buildTimeoutMs: 5_000,
    readyTimeoutMs: 2_000,
  };
}
const identify = `const fs = require('node:fs'); const os = require('node:os'); const path = require('node:path');
fs.writeFileSync('child.json', JSON.stringify({pid:process.pid, data:process.env.SAAA_SMOKE_DATA_DIR, marker:path.join(os.tmpdir(), 'saaa-frontend-'+process.env.SAAA_SMOKE_MARKER_ID+'.ready')}));`;
function assertClean(opts: SmokeOptions) {
  const child = JSON.parse(readFileSync(join(opts.root, "child.json"), "utf8"));
  expect(existsSync(child.data)).toBe(false);
  expect(existsSync(child.marker)).toBe(false);
  expect(() => process.kill(child.pid, 0)).toThrow();
}
function report(opts: SmokeOptions) {
  return JSON.parse(readFileSync(join(opts.reportDir, "summary.json"), "utf8"));
}

describe("desktop smoke process lifecycle", () => {
  test("reports ready, captures logs, and cleans up the live application and data", async () => {
    const opts = options(
      identify +
        `fs.writeSync(1, 'application fixture\\n'); fs.writeFileSync(JSON.parse(fs.readFileSync('child.json')).marker, 'ready'); setInterval(()=>{},1000);`,
    );
    await runDesktopSmoke(opts);
    expect(report(opts).status).toBe("ready");
    expect(readFileSync(join(opts.reportDir, "application.stdout.log"), "utf8")).toContain(
      "application fixture",
    );
    assertClean(opts);
  }, 15_000);
  test("times out without ready and cleans up", async () => {
    const opts = options(identify + "setInterval(()=>{},1000);");
    await expect(runDesktopSmoke(opts)).rejects.toThrow("deadline");
    expect(report(opts).stage).toBe("ready");
    expect(report(opts).status).toBe("failed");
    assertClean(opts);
  }, 15_000);
  test("reports early application exit and its stderr", async () => {
    const opts = options(identify + "console.error('fixture failure'); process.exit(7);");
    await expect(runDesktopSmoke(opts)).rejects.toThrow("code 7");
    expect(readFileSync(join(opts.reportDir, "application.stderr.log"), "utf8")).toContain(
      "fixture failure",
    );
    assertClean(opts);
  }, 15_000);
  test("preserves build failure diagnostics without launching the app", async () => {
    const opts = options(identify);
    opts.build = [process.execPath, "-e", "console.error('broken build'); process.exit(3)"];
    await expect(runDesktopSmoke(opts)).rejects.toThrow("code 3");
    expect(report(opts).stage).toBe("build");
    expect(readFileSync(join(opts.reportDir, "build.stderr.log"), "utf8")).toContain(
      "broken build",
    );
    expect(existsSync(join(opts.root, "child.json"))).toBe(false);
  }, 15_000);
  test("reports spawn and bundle failures", async () => {
    const opts = options(identify);
    opts.build = [join(opts.root, "missing-executable")];
    await expect(runDesktopSmoke(opts)).rejects.toThrow();
    expect(report(opts).status).toBe("failed");
    const bundle = options(identify);
    bundle.verifyBundle = async () => {
      throw new Error("invalid bundle");
    };
    await expect(runDesktopSmoke(bundle)).rejects.toThrow("invalid bundle");
    expect(report(bundle).stage).toBe("bundle");
  }, 15_000);
  test("terminates a timed-out build and its child process group", async () => {
    const opts = options(identify);
    opts.buildTimeoutMs = 2_000;
    opts.build = [
      process.execPath,
      "-e",
      `const {spawn}=require('node:child_process');const fs=require('node:fs');const c=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'inherit'});fs.writeFileSync('descendant',String(c.pid));setInterval(()=>{},1000);`,
    ];
    await expect(runDesktopSmoke(opts)).rejects.toThrow("timeout");
    const pid = Number(readFileSync(join(opts.root, "descendant"), "utf8"));
    // Reaping an orphan can lag the parent's close event slightly.
    for (let attempt = 0; attempt < 50; attempt++) {
      try {
        process.kill(pid, 0);
      } catch {
        return;
      }
      await Bun.sleep(20);
    }
    throw new Error("build descendant survived cleanup");
  }, 15_000);
  test("reports application spawn failure and removes its scratch directory", async () => {
    const opts = options(identify);
    opts.executable = [join(opts.root, "missing-application")];
    await expect(runDesktopSmoke(opts)).rejects.toThrow();
    expect(report(opts).status).toBe("failed");
    expect(report(opts).stage).toBe("ready");
  }, 15_000);
  test("forces cleanup when the application ignores SIGTERM", async () => {
    const opts = options(identify + "process.on('SIGTERM',()=>{}); setInterval(()=>{},1000);");
    await expect(runDesktopSmoke(opts)).rejects.toThrow("deadline");
    assertClean(opts);
  }, 15_000);
  test("redacts configured secrets, workspace paths and URLs in diagnostics", () => {
    const previous = process.env.SAAA_SMOKE_TEST_TOKEN;
    process.env.SAAA_SMOKE_TEST_TOKEN = "smoke-secret-fixture";
    try {
      const redacted = sanitizeSmokeLog(
        "smoke-secret-fixture /fixture/work/file https://host/path?token=x",
        "/fixture/work",
      );
      expect(redacted).toBe("[REDACTED] [WORKSPACE]/file [URL]");
    } finally {
      if (previous === undefined) delete process.env.SAAA_SMOKE_TEST_TOKEN;
      else process.env.SAAA_SMOKE_TEST_TOKEN = previous;
    }
  }, 15_000);
});
