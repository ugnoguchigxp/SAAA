import { expect, test } from "bun:test";
import { drainLarmVoice } from "../src/lib/larmVoiceDrain";
import type { LarmVoiceOwner } from "../src/lib/larmVoiceOwner";
const owner = (phase: LarmVoiceOwner["phase"]): LarmVoiceOwner => ({ id: "owner", conversationId: "conversation", phase });

test("stuck final-delivery state has a bounded drain deadline", async () => {
  let now = 0; let releases = 0;
  await drainLarmVoice(owner("ready"), {
    cancelled: () => false, busy: () => true,
    now: () => now, wait: async () => { now += 100; }, maxWaitMs: 500,
    release: async (_, drain) => { expect(drain).toBe(true); releases++; },
  });
  expect(now).toBe(500); expect(releases).toBe(1);
});

test("OFF during startup cancels immediately instead of waiting for capture to become idle", async () => {
  await drainLarmVoice(owner("starting"), {
    cancelled: () => false, busy: () => true,
    wait: async () => { throw new Error("startup must not wait"); },
    release: async (_, drain) => { expect(drain).toBe(false); },
  });
});

test("turning listening back on invalidates a pending drain", async () => {
  let cancelled = false; let releases = 0;
  await drainLarmVoice(owner("ready"), {
    cancelled: () => cancelled, busy: () => true,
    wait: async () => { cancelled = true; },
    release: async () => { releases++; },
  });
  expect(releases).toBe(0);
});
