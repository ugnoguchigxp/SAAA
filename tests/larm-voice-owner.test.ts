import { expect, test } from "bun:test";
import { LarmVoiceOwners } from "../src/lib/larmVoiceOwner";

test("capture restarts share one connection and stopping during startup rejects the stale capture", async () => {
  const calls: string[] = [];
  let resolve!: () => void;
  const gate = new Promise<void>((done) => {
    resolve = done;
  });
  const owners = new LarmVoiceOwners(
    async (name) => {
      calls.push(name);
      if (name === "begin_larm_voice_session") await gate;
    },
    () => "owner-1",
  );
  const owner = owners.own("conversation-1");
  const first = owners.prepare("conversation-1");
  const second = owners.prepare("conversation-1");
  expect(owners.own("conversation-1")).toBe(owner);
  const end = owners.end(owner);
  resolve();
  await end;
  const results = await Promise.allSettled([first, second]);
  expect(results.every((value) => value.status === "rejected")).toBe(true);
  expect(calls).toEqual([
    "begin_larm_voice_session",
    "end_larm_voice_session",
    "end_larm_voice_session",
  ]);
});

test("late failure releases the original owner without touching its replacement", async () => {
  const calls: [string, Record<string, unknown>][] = [];
  let index = 0;
  const owners = new LarmVoiceOwners(
    async (name, args) => {
      calls.push([name, args]);
    },
    () => `owner-${++index}`,
  );
  const old = owners.own("conversation-1");
  await owners.prepare("conversation-1");
  await owners.end(old, true);
  const next = owners.own("conversation-1");
  await owners.prepare("conversation-1");
  await owners.fail(old);
  expect(owners.current()).toBe(next);
  await owners.prepare("conversation-1");
  expect(calls.filter(([name]) => name === "begin_larm_voice_session")).toHaveLength(2);
  expect(calls.at(-1)?.[1].ownerId).toBe(next.id);
  expect(calls[1][1].drain).toBe(true);
});

test("failed startup can be retried after releasing the failed connection", async () => {
  let failures = 1;
  const owners = new LarmVoiceOwners(
    async (name) => {
      if (name === "begin_larm_voice_session" && failures-- > 0) throw new Error("startup failed");
    },
    () => "owner",
  );
  const owner = owners.own("conversation");
  await expect(owners.prepare("conversation")).rejects.toThrow("startup failed");
  await owners.fail(owner);
  owners.own("conversation");
  await owners.prepare("conversation");
});

test("out-of-order start completion is released even if an earlier end saw no backend owner", async () => {
  let finishStart!: () => void;
  let backendOwner: string | null = null;
  const gate = new Promise<void>((resolve) => {
    finishStart = resolve;
  });
  const owners = new LarmVoiceOwners(
    async (name, args) => {
      if (name === "begin_larm_voice_session") {
        await gate;
        backendOwner = args.ownerId as string;
      } else if (backendOwner === args.ownerId) backendOwner = null;
    },
    () => "owner",
  );
  const owner = owners.own("conversation");
  const pending = owners.prepare("conversation");
  const outcome = pending.catch(() => undefined);
  const end = owners.end(owner);
  finishStart();
  await Promise.all([outcome, end]);
  expect(backendOwner).toBeNull();
});

test("release failure is retried before a replacement can start", async () => {
  let index = 0;
  let failRelease = true;
  const calls: string[] = [];
  const owners = new LarmVoiceOwners(
    async (name, args) => {
      calls.push(`${name}:${args.ownerId}`);
      if (name === "end_larm_voice_session" && failRelease) throw new Error("release failed");
    },
    () => `owner-${++index}`,
  );
  const first = owners.own("conversation");
  await owners.prepare("conversation");
  await expect(owners.end(first)).rejects.toThrow("release failed");
  owners.own("replacement");
  await expect(owners.prepare("replacement")).rejects.toThrow("release failed");
  expect(calls.some((c) => c === "begin_larm_voice_session:owner-2")).toBe(false);
  failRelease = false;
  await owners.prepare("replacement");
  expect(calls.at(-1)).toBe("begin_larm_voice_session:owner-2");
});

test("a stale capture for another conversation never gets the new owner's lease", async () => {
  const calls: string[] = [];
  const owners = new LarmVoiceOwners(
    async (name) => {
      calls.push(name);
    },
    () => "owner",
  );
  const owner = owners.own("new");
  await expect(owners.prepare("old")).rejects.toThrow("asr-cancelled");
  expect(owners.current()).toBe(owner);
  expect(calls).toHaveLength(0);
});

test("replacing an owner directly still releases the previous connection first", async () => {
  const calls: string[] = [];
  let id = 0;
  const owners = new LarmVoiceOwners(
    async (name, args) => {
      calls.push(`${name}:${args.ownerId}`);
    },
    () => `owner-${++id}`,
  );
  owners.own("old");
  await owners.prepare("old");
  owners.own("new");
  await owners.prepare("new");
  expect(calls).toEqual([
    "begin_larm_voice_session:owner-1",
    "end_larm_voice_session:owner-1",
    "begin_larm_voice_session:owner-2",
  ]);
});
