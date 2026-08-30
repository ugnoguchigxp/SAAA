import { describe, expect, test } from "bun:test";
import type { MutableRefObject } from "react";
import { initialConversationSession } from "../src/lib/conversationSession";
import { initialVoiceSession, transitionVoiceSession } from "../src/lib/voiceSession";
import { drainVoiceSegmentQueue } from "../src/features/voice/voiceSegmentProcessor";
import { VoiceSegmentQueue } from "../src/features/voice/voiceSegmentQueue";

describe("ambient voice segment processing", () => {
  test("delivers a completed transcript after capture has already stopped", async () => {
    const queue = new VoiceSegmentQueue();
    const samples = new Float32Array([0.1, -0.1]);
    queue.push({
      conversationId: "conversation-1",
      samples,
      sampleRate: 16_000,
      ttsActiveAtCapture: false,
    });
    const session = { current: initialVoiceSession } as MutableRefObject<typeof initialVoiceSession>;
    const submitted: string[] = [];

    await drainVoiceSegmentQueue({
      queue,
      session,
      disposed: { current: false },
      conversation: { current: initialConversationSession },
      pendingPrompts: { current: [] },
      applyEvent: (event) => {
        session.current = transitionVoiceSession(session.current, event);
      },
      setTranscript: () => undefined,
      setError: (message) => { throw new Error(message); },
      setRuntimeActivity: () => undefined,
      stopSpeech: async () => undefined,
      submitPrompt: async (prompt) => { submitted.push(prompt); },
      transcribe: async () => "取得済みの発話",
    });

    expect(submitted).toEqual(["取得済みの発話"]);
    expect(queue.length).toBe(0);
    expect(Array.from(samples)).toEqual([0, 0]);
  });
});
