import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startInputWatch } from "../scripts/verify-input-watch";

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for the watcher");
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test("reverting a file still advances the watcher generation", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-watch-"));
  const file = join(directory, "note.txt");
  writeFileSync(file, "one");
  spawnSync("git", ["init", "-q"], { cwd: directory });
  const watch = startInputWatch(directory, { exclude: () => false });
  try {
    expect(watch.available).toBe(true);
    expect(watch.begin().ok).toBe(true);
    writeFileSync(file, "two");
    writeFileSync(file, "one");
    await until(() => watch.generation() >= 1);
    const scanned = watch.finalScan();
    expect(scanned.scanned).toBe(true);
    expect(scanned.overflow).toBe(false);
    expect(scanned.limitation.length).toBeGreaterThan(0);
  } finally {
    watch.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("git metadata and the shared target are outside the watcher", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-watch-"));
  mkdirSync(join(directory, ".git"));
  mkdirSync(join(directory, "src-tauri/target"), { recursive: true });
  const source = join(directory, "src.txt");
  writeFileSync(source, "kept");
  const watch = startInputWatch(directory);
  try {
    await new Promise((resolve) => setTimeout(resolve, 50));
    const baseline = watch.generation();
    writeFileSync(join(directory, ".git/config"), "ignored");
    writeFileSync(join(directory, "src-tauri/target/out"), "ignored");
    await new Promise((resolve) => setTimeout(resolve, 150));
    expect(watch.generation()).toBe(baseline);
    writeFileSync(source, "changed");
    await until(() => watch.generation() > baseline);
  } finally {
    watch.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("watcher overflow and an unavailable root do not report success", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-watch-"));
  const file = join(directory, "a.txt");
  writeFileSync(file, "0");
  const watch = startInputWatch(directory, { maxEvents: 1, exclude: () => false });
  try {
    writeFileSync(file, "1");
    await until(() => watch.overflow());
    expect(watch.failed()).toBe(false);
    expect(watch.detail().overflow).toBe(true);
  } finally {
    watch.close();
    rmSync(directory, { recursive: true, force: true });
  }
  const missing = startInputWatch(join(directory, "missing"));
  expect(missing.available).toBe(false);
  expect(missing.failed()).toBe(true);
  missing.close();
});

for (const operation of ["edit", "add", "delete", "chmod", "link"] as const) {
  test(`final walk detects ${operation} with event delivery disabled`, () => {
    const directory = mkdtempSync(join(tmpdir(), "saaa-watch-scan-"));
    spawnSync("git", ["init", "-q"], { cwd: directory });
    const file = join(directory, "a.txt");
    writeFileSync(file, "original");
    const watch = startInputWatch(directory, { exclude: () => true });
    try {
      expect(watch.begin().ok).toBe(true);
      if (operation === "edit") writeFileSync(file, "modified");
      if (operation === "add") writeFileSync(join(directory, "b.txt"), "added");
      if (operation === "delete") rmSync(file);
      if (operation === "chmod") chmodSync(file, 0o755);
      if (operation === "link") {
        rmSync(file);
        writeFileSync(join(directory, "b.txt"), "original");
        symlinkSync("b.txt", file);
      }
      const detail = watch.finalScan();
      expect(detail.scanned).toBe(true);
      expect(detail.scanMatched).toBe(false);
      expect(detail.generationStart).toBe(0);
      expect(detail.generationEnd).toBe(0);
    } finally {
      watch.close();
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

test("tracked files in an excluded directory still report edit-and-revert events", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-watch-tracked-"));
  spawnSync("git", ["init", "-q"], { cwd: directory });
  mkdirSync(join(directory, "node_modules"));
  const file = join(directory, "node_modules/input.txt");
  writeFileSync(file, "one");
  expect(spawnSync("git", ["add", "-f", "node_modules/input.txt"], { cwd: directory }).status).toBe(
    0,
  );
  const watch = startInputWatch(directory);
  try {
    expect(watch.begin().ok).toBe(true);
    writeFileSync(file, "two");
    writeFileSync(file, "one");
    await until(() => watch.generation() > 0);
    expect(watch.finalScan().scanMatched).toBe(true);
    expect(watch.detail().generationEnd).toBeGreaterThan(watch.detail().generationStart);
  } finally {
    watch.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("close does not invent a completed scan and resets apply per attempt", () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-watch-close-"));
  spawnSync("git", ["init", "-q"], { cwd: directory });
  const watch = startInputWatch(directory);
  try {
    expect(watch.begin().ok).toBe(true);
    expect(watch.finalScan().scanned).toBe(true);
    watch.begin();
    watch.close();
    expect(watch.detail().scanned).toBe(false);
    expect(watch.finalScan().failed).toBe(true);
  } finally {
    watch.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
