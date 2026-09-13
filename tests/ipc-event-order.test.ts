import { expect, test } from "bun:test";
import { runtimeEventOrder, voiceAsrEventOrder, meetingEventOrder } from "../src/lib/ipcEventOrder";
import { guardedReceiver, runtimeEventSchema, voiceAsrEventSchema } from "../src/lib/ipcValidation";
import type { VoiceAsrStreamEvent } from "../src/lib/generated/voiceAsr";

const ready = {
  type: "ready",
  sessionId: "s",
  currentUtteranceId: "u",
  protocol: "native",
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

test("meeting scopes sequences to session and lane without rejecting out-of-order completion", () => {
  const order = meetingEventOrder();
  expect(order({ type: "stateChanged", sessionId: null, state: "idle" })).toBe(true);
  expect(order({ type: "stateChanged", sessionId: "m", state: "active" })).toBe(true);
  const transcript = {
    type: "transcriptFinal",
    sessionId: "m",
    lane: "microphone",
    sequence: 1,
    text: "ok",
    language: null,
  } as const;
  expect(order(transcript)).toBe(true);
  expect(order({ ...transcript, sequence: 0 })).toBe(true);
  expect(order({ ...transcript, lane: "system-audio" })).toBe(true);
  expect(order(transcript)).toBe(false);
  expect(order({ type: "stateChanged", sessionId: "m", state: "completed" })).toBe(true);
  expect(order({ ...transcript, sequence: 2 })).toBe(false);
  expect(order({ type: "stateChanged", sessionId: "m", state: "active" })).toBe(false);
  expect(order({ type: "stateChanged", sessionId: "next", state: "active" })).toBe(true);
  expect(order({ ...transcript, sessionId: "next" })).toBe(true);
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

test("meeting preflight can follow idle while failure seals the active session", () => {
  const order = meetingEventOrder();
  expect(order({ type: "stateChanged", sessionId: null, state: "idle" })).toBe(true);
  expect(order({ type: "stateChanged", sessionId: null, state: "ready" })).toBe(true);
  expect(order({ type: "stateChanged", sessionId: "m", state: "active" })).toBe(true);
  expect(
    order({
      type: "failed",
      sessionId: "m",
      code: "MEETING_ASR_FAILED",
      message: "failed",
      recovery: "retry",
    }),
  ).toBe(true);
  expect(
    order({
      type: "transcriptFinal",
      sessionId: "m",
      lane: "microphone",
      sequence: 0,
      text: "late",
      language: null,
    }),
  ).toBe(false);
});

test("meeting subscription accepts a failure notice after its initial failed snapshot", () => {
  const order = meetingEventOrder();
  expect(order({ type: "stateChanged", sessionId: "m", state: "failed" })).toBe(true);
  expect(
    order({
      type: "failed",
      sessionId: "m",
      code: "MEETING_ASR_FAILED",
      message: "failed",
      recovery: "retry",
    }),
  ).toBe(true);
});
