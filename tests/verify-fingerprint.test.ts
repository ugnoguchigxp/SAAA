import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  captureInputFingerprint,
  scanVerificationInputs,
  pathsFromPorcelain,
  staleDecision,
  type InputFingerprint,
} from "../scripts/verify-fingerprint";

const measured = (value: string): InputFingerprint => ({ measured: true, value });
const unmeasured: InputFingerprint = { measured: false, value: "" };

test("a changed input retries once and then refuses OK", () => {
  expect(staleDecision(measured("a"), measured("a"), 1)).toBe("ok");
  expect(staleDecision(measured("a"), measured("b"), 1)).toBe("retry");
  expect(staleDecision(measured("b"), measured("c"), 2)).toBe("stale");
});

test("an unreadable fingerprint is not treated as proof of an unchanged tree", () => {
  expect(staleDecision(unmeasured, measured("a"), 1)).toBe("unmeasured");
  expect(staleDecision(measured("a"), unmeasured, 2)).toBe("unmeasured");
});

test("staged rename porcelain measures the destination, not only the old path", () => {
  const status = Buffer.from("R  new.txt\0old.txt\0");
  expect(pathsFromPorcelain(status)).toEqual(["new.txt"]);
});

test("editing a staged rename destination changes the fingerprint", () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-fingerprint-rename-"));
  const git = (...args: string[]) => {
    const result = spawnSync("git", args, { cwd: directory });
    expect(result.status).toBe(0);
  };
  try {
    git("init", "-q");
    writeFileSync(join(directory, "old.txt"), "old");
    git("add", "old.txt");
    git("-c", "user.email=test@example.com", "-c", "user.name=test", "commit", "-q", "-m", "init");
    git("mv", "old.txt", "new.txt");
    const status = spawnSync("git", ["status", "--porcelain=v1", "-z"], { cwd: directory });
    expect(pathsFromPorcelain(Buffer.from(status.stdout))).toContain("new.txt");
    const before = captureInputFingerprint(directory);
    writeFileSync(join(directory, "new.txt"), "edited destination");
    const after = captureInputFingerprint(directory);
    expect(before.measured).toBe(true);
    expect(after.measured).toBe(true);
    expect(after.value).not.toBe(before.value);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("walk covers ignored inputs and preserves the closed exclusions with tracked priority", () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-fingerprint-scan-"));
  const git = (...args: string[]) =>
    expect(spawnSync("git", args, { cwd: directory }).status).toBe(0);
  try {
    git("init", "-q");
    for (const path of [
      "node_modules",
      "nested/target",
      "src-tauri/gen/schemas",
      "src-tauri/gen/other",
      "nested",
    ])
      mkdirSync(join(directory, path), { recursive: true });
    writeFileSync(join(directory, ".gitignore"), "*.secret\n");
    writeFileSync(join(directory, "ignored.secret"), "one");
    writeFileSync(join(directory, "node_modules/tracked.txt"), "one");
    git("add", "-f", "node_modules/tracked.txt");
    const scan = () => {
      const value = scanVerificationInputs(directory);
      expect(value.ok).toBe(true);
      return value.ok ? value.digest : "";
    };
    let before = scan();
    for (const path of [
      "ignored.secret",
      "nested/target/input",
      "nested/app.tsbuildinfo",
      "src-tauri/gen/other/input",
      "node_modules/tracked.txt",
    ]) {
      writeFileSync(join(directory, path), "changed");
      const after = scan();
      expect(after).not.toBe(before);
      before = after;
    }
    for (const path of [
      "node_modules/output",
      "src-tauri/gen/schemas/output",
      "tsconfig.tsbuildinfo",
    ])
      writeFileSync(join(directory, path), "excluded");
    expect(scan()).toBe(before);
    symlinkSync("/does-not-exist", join(directory, "node_modules/excluded-link"));
    expect(scan()).toBe(before);
    chmodSync(join(directory, "ignored.secret"), 0o744);
    before = scan();
    chmodSync(join(directory, "ignored.secret"), 0o754);
    expect(scan()).not.toBe(before);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("walk rejects symlink escapes, unresolved links, and indirect escapes", () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-fingerprint-links-"));
  const outside = mkdtempSync(join(tmpdir(), "saaa-fingerprint-outside-"));
  try {
    expect(spawnSync("git", ["init", "-q"], { cwd: directory }).status).toBe(0);
    symlinkSync(outside, join(directory, "escape"));
    symlinkSync("escape", join(directory, "indirect"));
    expect(scanVerificationInputs(directory).ok).toBe(false);
    rmSync(join(directory, "escape"));
    expect(scanVerificationInputs(directory).ok).toBe(false);
    rmSync(join(directory, "indirect"));
    symlinkSync("absent", join(directory, "dangling"));
    expect(scanVerificationInputs(directory).ok).toBe(false);
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(outside, { recursive: true, force: true });
  }
});
