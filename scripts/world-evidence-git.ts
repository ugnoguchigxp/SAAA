import { lstatSync, readFileSync, readlinkSync } from "node:fs";
import { resolve } from "node:path";

const EXCLUDED = [
  ":(exclude)spec/evidence/world-delivery/remaining-report.json",
  ":(exclude)spec/evidence/world-model/m4a-report.json",
];

function git(repoPath: string, args: string[]): Uint8Array {
  const result = Bun.spawnSync(["git", ...args], { cwd: repoPath, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    const detail = result.stderr.toString().trim();
    throw new Error(`git ${args[0]} failed${detail ? `: ${detail}` : ""}`);
  }
  return result.stdout;
}

function snapshot(repoPath: string): { revision: string; digest: string } {
  const paths = ["--", ".", ...EXCLUDED];
  const revision = git(repoPath, ["rev-parse", "HEAD"]).toString().trim();
  if (!/^[0-9a-f]{40}([0-9a-f]{24})?$/.test(revision))
    throw new Error("git rev-parse returned an invalid revision");
  const hasher = new Bun.CryptoHasher("sha256");
  hasher.update("tracked-diff\0");
  hasher.update(git(repoPath, ["diff", "--binary", "HEAD", ...paths]));
  hasher.update("\0worktree-status\0");
  hasher.update(git(repoPath, ["status", "--short", "--untracked-files=all", ...paths]));
  const untracked = git(repoPath, ["ls-files", "--others", "--exclude-standard", ...paths])
    .toString()
    .split("\n")
    .filter(Boolean)
    .sort();
  for (const path of untracked) {
    const absolute = resolve(repoPath, path);
    hasher.update(`\0untracked\0${path}\0`);
    const stat = lstatSync(absolute);
    if (stat.isSymbolicLink()) hasher.update(`symlink\0${readlinkSync(absolute)}`);
    else if (stat.isFile()) hasher.update(readFileSync(absolute));
    else throw new Error(`unsupported untracked evidence path: ${path}`);
  }
  return { revision, digest: hasher.digest("hex") };
}

export function gitEvidence(repoPath: string): { revision: string; digest: string } {
  const first = snapshot(repoPath);
  const second = snapshot(repoPath);
  if (first.revision !== second.revision || first.digest !== second.digest)
    throw new Error("worktree changed while capturing World evidence identity");
  return second;
}
