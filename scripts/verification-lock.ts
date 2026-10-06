import { Database } from "bun:sqlite";
import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

type Owner = { token: string; pid: number; group?: number };
export type VerificationLock = {
  env: NodeJS.ProcessEnv;
  inherited: boolean;
  track: (pid?: number) => void;
  release: () => void;
};

function live(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (cause) {
    return (cause as NodeJS.ErrnoException).code !== "ESRCH";
  }
}

export function verificationGroupLive(pid?: number): boolean {
  if (!pid) return false;
  if (process.platform === "win32") return live(pid);
  const result = spawnSync("ps", ["-axo", "pgid=,stat="], { encoding: "utf8" });
  if (result.status !== 0) throw new Error("Could not inspect the previous build process group");
  return result.stdout.split("\n").some((line) => {
    const [group, state] = line.trim().split(/\s+/);
    return Number(group) === pid && state && !state.startsWith("Z");
  });
}

/** Git worktrees, package subdirectories and alternate target dirs share one lock. */
function lockFiles(cwd: string): { database: string; owner: string } {
  const git = spawnSync("git", ["rev-parse", "--git-common-dir"], {
    cwd,
    encoding: "utf8",
  });
  const identity = realpathSync(git.status === 0 ? resolve(cwd, git.stdout.trim()) : cwd);
  const directory = join(identity, "saaa-verification-lock");
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  return { database: join(directory, "lock.sqlite"), owner: join(directory, "owner.json") };
}

function readOwner(path: string): Owner | undefined {
  try {
    const owner: Owner = JSON.parse(readFileSync(path, "utf8"));
    if (typeof owner.token === "string" && Number.isSafeInteger(owner.pid) && owner.pid > 0)
      return owner;
  } catch (cause) {
    if ((cause as NodeJS.ErrnoException).code !== "ENOENT" && !(cause instanceof SyntaxError))
      throw cause;
  }
}

export async function acquireVerificationLock(
  cwd: string,
  signal: AbortSignal,
): Promise<VerificationLock> {
  const files = lockFiles(cwd);
  const token = randomUUID();
  let db: Database | undefined;
  while (true) {
    signal.throwIfAborted();
    const owner = readOwner(files.owner);
    if (
      process.env.SAAA_VERIFY_LOCK_PATH === files.database &&
      process.env.SAAA_VERIFY_LOCK_TOKEN === owner?.token &&
      owner &&
      (live(owner.pid) || verificationGroupLive(owner.group))
    ) {
      return { env: process.env, inherited: true, track: () => {}, release: () => {} };
    }
    try {
      db = new Database(files.database, { create: true });
      db.exec("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE");
      // SIGKILL releases SQLite's OS lock, but a compiler descendant may still be running.
      // Do not start another build until that recorded process group has also exited.
      if (verificationGroupLive(readOwner(files.owner)?.group)) {
        db.close(true);
        db = undefined;
      } else {
        break;
      }
    } catch (cause) {
      db?.close();
      db = undefined;
      if ((cause as { code?: string }).code !== "SQLITE_BUSY") throw cause;
    }
    await Bun.sleep(100);
  }
  const held = db!;
  const owner: Owner = { token, pid: process.pid };
  const save = () => {
    const temporary = `${files.owner}.${token}`;
    writeFileSync(temporary, JSON.stringify(owner), { mode: 0o600 });
    renameSync(temporary, files.owner);
  };
  try {
    save();
  } catch (cause) {
    held.close(true);
    throw cause;
  }
  return {
    env: { ...process.env, SAAA_VERIFY_LOCK_PATH: files.database, SAAA_VERIFY_LOCK_TOKEN: token },
    inherited: false,
    track: (pid) => {
      owner.group = pid;
      save();
    },
    release: () => {
      // Never unlink the database: waiters must continue locking the same inode.
      try {
        owner.token = "";
        save();
      } finally {
        held.close(true);
      }
    },
  };
}
