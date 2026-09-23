import { expect, test } from "bun:test";
import type { VoiceSettings } from "../src/lib/contracts";
import { idleCaptureShouldStart } from "../src/features/voice/idleVoiceCapture";

const ready = {
  listeningEnabled: true,
  selectedConversationId: "c1",
  voiceSettings: {} as VoiceSettings,
  situationHold: false,
  speechRunId: null as string | null,
  capture: "idle",
  hasStream: false,
  actionInProgress: false,
};

test("idle capture starts only when listening is idle and unblocked", () => {
  expect(idleCaptureShouldStart(ready)).toBe(true);
  expect(idleCaptureShouldStart({ ...ready, situationHold: true })).toBe(false);
  expect(idleCaptureShouldStart({ ...ready, capture: "recording" })).toBe(false);
  expect(idleCaptureShouldStart({ ...ready, hasStream: true })).toBe(false);
  expect(idleCaptureShouldStart({ ...ready, actionInProgress: true })).toBe(false);
});
