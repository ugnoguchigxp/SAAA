import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readDesktopE2EChecks } from "../scripts/desktop-e2e-report";

const directories: string[] = [];

afterEach(() => {
  for (const directory of directories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

function marker(checks: Record<string, boolean>): string {
  const directory = mkdtempSync(join(tmpdir(), "saaa-e2e-report-test-"));
  directories.push(directory);
  const path = join(directory, "ready.json");
  writeFileSync(path, JSON.stringify({ status: "passed", checks }));
  return path;
}

describe("desktop E2E report", () => {
  test("returns structured checks when every required check passed", () => {
    expect(
      readDesktopE2EChecks(marker({ frontendRendered: true, ipcReady: true }), [
        "frontendRendered",
        "ipcReady",
      ]),
    ).toEqual({ frontendRendered: true, ipcReady: true });
  });

  test("rejects a required check that did not pass", () => {
    expect(() =>
      readDesktopE2EChecks(marker({ frontendRendered: true, ipcReady: false }), [
        "frontendRendered",
        "ipcReady",
      ]),
    ).toThrow("ipcReady");
  });
});
