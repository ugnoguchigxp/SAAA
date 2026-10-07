import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { isScanExcluded, scanRepository } from "../scripts/verify-input-scan";
import { startInputWatch } from "../scripts/verify-input-watch";

test("the closed exclusion list keeps tracked files and does not hide nested inputs", () => {
  expect(isScanExcluded(".git/HEAD")).toBe(true);
  expect(isScanExcluded("src-tauri/target/debug/a")).toBe(true);
  expect(isScanExcluded("src-tauri/gen/schemas/acl.json")).toBe(true);
  expect(isScanExcluded("tsconfig.tsbuildinfo")).toBe(true);
  expect(isScanExcluded("src-tauri/gen/other.json")).toBe(false);
  expect(isScanExcluded("crates/demo/target/a.rlib")).toBe(false);
  expect(isScanExcluded("nested/app.tsbuildinfo")).toBe(false);
  const directory = mkdtempSync(join(tmpdir(), "saaa-scan-exclude-"));
  try {
    mkdirSync(join(directory, "node_modules"));
    mkdirSync(join(directory, "crates/demo/target"), { recursive: true });
    writeFileSync(join(directory, "node_modules/ignored.txt"), "no");
    writeFileSync(join(directory, "node_modules/kept.txt"), "yes");
    writeFileSync(join(directory, "crates/demo/target/a.rlib"), "kept");
    const ignored = scanRepository(directory, new Set());
    const tracked = scanRepository(directory, new Set(["node_modules/kept.txt"]));
    expect(ignored.ok && tracked.ok).toBe(true);
    if (ignored.ok && tracked.ok) expect(tracked.digest).not.toBe(ignored.digest);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a symlink that leaves the repository makes the scan fail", () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-scan-link-"));
  const outside = mkdtempSync(join(tmpdir(), "saaa-scan-outside-"));
  try {
    symlinkSync(outside, join(directory, "escape"));
    const scan = scanRepository(directory, new Set());
    expect(scan.ok).toBe(false);
    if (!scan.ok) expect(scan.reason).toContain("symlink");
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(outside, { recursive: true, force: true });
  }
});

test("stopping event delivery still leaves content edits in the final scan", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-scan-silent-"));
  try {
    spawnSync("git", ["init", "-q"], { cwd: directory });
    writeFileSync(join(directory, "a.txt"), "one");
    const watch = startInputWatch(directory, { exclude: () => true });
    try {
      expect(watch.begin().ok).toBe(true);
      writeFileSync(join(directory, "a.txt"), "two");
      writeFileSync(join(directory, "b.txt"), "added");
      rmSync(join(directory, "a.txt"));
      await new Promise((resolve) => setTimeout(resolve, 50));
      const end = watch.finalScan();
      expect(watch.generation()).toBe(0);
      expect(end.scanned).toBe(true);
      expect(end.scanMatched).toBe(false);
      expect(end.limitation).toContain("開始・終了scan");
      const scanned = end.scanned;
      watch.close();
      expect(watch.detail().scanned).toBe(scanned);
    } finally {
      watch.close();
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
