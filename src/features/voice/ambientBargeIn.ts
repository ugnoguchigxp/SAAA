import type { MutableRefObject } from "react";

const BARGE_IN_HOLD_MS = 200;
const BARGE_IN_GRACE_MS = 400;

export function considerBargeIn(
  input: {
    bargeInEnabled?: boolean;
    speechIsPlaying?: () => boolean;
    ttsStartedAtMs?: () => number;
    interruptSpeech?: () => void;
    speechRunId?: () => string | null;
    bargeInSpeechSince?: MutableRefObject<number>;
    bargeInFiredFor?: MutableRefObject<string | null>;
  },
  hasSpeech: boolean,
): void {
  const since = input.bargeInSpeechSince;
  const fired = input.bargeInFiredFor;
  if (!since || !fired) return;
  const runId = input.speechRunId?.() ?? null;
  const now = performance.now();
  const eligible =
    hasSpeech &&
    input.bargeInEnabled !== false &&
    Boolean(runId) &&
    Boolean(input.speechIsPlaying?.()) &&
    (input.ttsStartedAtMs?.() ?? 0) + BARGE_IN_GRACE_MS <= now;
  if (!eligible || !runId) {
    since.current = 0;
    return;
  }
  if (fired.current === runId) return;
  if (since.current === 0) since.current = now;
  if (now - since.current < BARGE_IN_HOLD_MS) return;
  fired.current = runId;
  since.current = 0;
  input.interruptSpeech?.();
}
