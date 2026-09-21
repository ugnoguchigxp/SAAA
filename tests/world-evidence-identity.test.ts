import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { worldEvidenceIdentity } from "../scripts/world-evidence-identity";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true });
});

test("world evidence digest includes untracked file contents", () => {
  const directory = mkdtempSync(join(tmpdir(), "world-evidence-"));
  directories.push(directory);
  const git = (...args: string[]) => {
    const result = Bun.spawnSync(["git", ...args], { cwd: directory, stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(result.stderr.toString());
  };
  git("init", "-q");
  git("config", "user.email", "world-test@example.invalid");
  git("config", "user.name", "World Test");
  writeFileSync(join(directory, "tracked.txt"), "base\n");
  git("add", "tracked.txt");
  git("commit", "-qm", "base");
  writeFileSync(join(directory, "new.txt"), "first\n");
  const first = worldEvidenceIdentity(directory);
  writeFileSync(join(directory, "new.txt"), "second\n");
  const second = worldEvidenceIdentity(directory);
  expect(first.code_revision).toBe(second.code_revision);
  expect(first.dirty_diff_digest).not.toBe(second.dirty_diff_digest);
});
