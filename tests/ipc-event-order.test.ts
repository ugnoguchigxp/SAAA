import { expect, test } from "bun:test";
import { runtimeEventOrder, voiceAsrEventOrder } from "../src/lib/ipcEventOrder";
import { guardedReceiver, runtimeEventSchema, voiceAsrEventSchema } from "../src/lib/ipcValidation";
import type { VoiceAsrStreamEvent } from "../src/lib/generated/voiceAsr";

const ready = {
  type: "ready",
  sessionId: "s",
  currentUtteranceId: "u",
  protocol: "batch-agreement",
  scope: "all-speakers",
} as const;
const final = (utteranceId = "u", revision = 1): VoiceAsrStreamEvent => ({
  type: "final",
  sessionId: "s",
  utteranceId,
  revision,
  startMs: 0,
  endMs: 10,
  text: "ok",
  language: null,
});

test("runtime quarantines output after a terminal and preserves asynchronous speech and fallback", () => {
  const order = runtimeEventOrder("r");
  expect(
    order({ type: "started", runId: "r", route: "conversation.respond", providerId: "a" }),
  ).toBe(true);
  expect(
    order({ type: "providerFailed", runId: "r", providerId: "a", reason: "unavailable" }),
  ).toBe(true);
  expect(
    order({ type: "started", runId: "r", route: "conversation.respond", providerId: "b" }),
  ).toBe(true);
  const received: string[] = [];
  let failures = 0;
  const receive = guardedReceiver(
    runtimeEventSchema,
    "runtime",
    (event) => received.push(event.type),
    order,
    () => failures++,
  );
  receive({ type: "cancelled", runId: "r" });
  receive({ type: "speechEnded", runId: "r" });
  receive({ type: "delta", runId: "r", text: "must not reach UI" });
  receive({ type: "cancelled", runId: "r" });
  expect(received).toEqual(["cancelled", "speechEnded"]);
  expect(failures).toBe(1);
});

test("ASR supports interleaved utterances and rejects stale revisions and repeated terminals", () => {
  const order = voiceAsrEventOrder("s");
  expect(order(ready)).toBe(true);
  expect(
    order({
      type: "partial",
      sessionId: "s",
      utteranceId: "u",
      revision: 1,
      startMs: 0,
      endMs: 5,
      stableText: "",
      unstableText: "ok",
      language: null,
    }),
  ).toBe(true);
  expect(order(final("next"))).toBe(true);
  expect(order(final("u", 2))).toBe(true);
  expect(order(final("u", 3))).toBe(false);
  expect(order(ready)).toBe(false);
  const stale = voiceAsrEventOrder("s");
  expect(stale(final())).toBe(false);
  expect(stale(ready)).toBe(true);
  const partial = {
    type: "partial",
    sessionId: "s",
    utteranceId: "u",
    revision: 2,
    startMs: 0,
    endMs: 5,
    stableText: "",
    unstableText: "ok",
    language: null,
  } as const;
  expect(stale(partial)).toBe(true);
  expect(stale({ ...partial, revision: 1 })).toBe(false);
});

test("ASR stopped seals the channel without forwarding late finals", () => {
  const received: string[] = [];
  let failures = 0;
  const receive = guardedReceiver(
    voiceAsrEventSchema,
    "voice-asr",
    (event) => received.push(event.type),
    voiceAsrEventOrder("s"),
    () => failures++,
  );
  receive(ready);
  receive({ type: "stopped", sessionId: "s" });
  receive(final());
  receive(ready);
  expect(received).toEqual(["ready", "stopped"]);
  expect(failures).toBe(1);
});

test("fatal ASR failure permits cleanup but rejects further transcripts", () => {
  const order = voiceAsrEventOrder("s");
  expect(order(ready)).toBe(true);
  expect(
    order({
      type: "failed",
      sessionId: "s",
      utteranceId: "u",
      code: "asr-stream-protocol",
      message: "failed",
      recovery: "retry",
      fatal: true,
    }),
  ).toBe(true);
  expect(order(final())).toBe(false);
  expect(
    order({ type: "utteranceDiscarded", sessionId: "s", utteranceId: "u", reason: "cancelled" }),
  ).toBe(true);
  expect(order({ type: "stopped", sessionId: "s" })).toBe(true);
});
