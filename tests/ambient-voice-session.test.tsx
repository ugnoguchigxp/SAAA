import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../src/lib/contracts";
import {
  initialConversationSession,
  type ConversationSession,
  type PendingConversationPrompt,
} from "../src/lib/conversationSession";
import { channels, invokeCalls, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

await import("../src/i18n");
const { effectiveCaptureSettings, useAmbientVoiceSession } =
  await import("../src/features/voice/useAmbientVoiceSession");

class FakeAudioWorkletNode {
  port = {
    onmessage: null as ((event: MessageEvent) => void) | null,
    postMessage: () => undefined,
  };
  connect() {
    return this;
  }
  disconnect() {
    return this;
  }
}

class FakeAudioContext {
  sampleRate = 16_000;
  state = "running";
  destination = {};
  audioWorklet = { addModule: async () => undefined };
  constructor(options?: { sampleRate?: number }) {
    if (options?.sampleRate) this.sampleRate = options.sampleRate;
  }
  createMediaStreamSource() {
    return { connect: () => this, disconnect: () => undefined };
  }
  close = async () => {
    this.state = "closed";
  };
  resume = async () => {
    this.state = "running";
  };
}

const voiceSettings: VoiceSettings = {
  listeningEnabled: false,
  inputDeviceId: "default",
  outputDeviceId: "default",
  vadSensitivity: "medium",
  silenceTimeoutMs: 1_500,
  allowedLanguages: ["ja"],
  autoSpeak: true,
};

const voicePolicy: ConversationVoicePolicySnapshot = {
  conversationId: "conversation-1",
  speechOutput: "inherit",
  listeningPace: "balanced",
  policyRevision: 1,
  updatedAt: "1",
  effectiveSpeechOutput: "speak",
  speechReasonCode: "global_default",
  effectiveListeningPace: "balanced",
  effectiveSilenceTimeoutMs: 1_500,
};

type SessionApi = ReturnType<typeof useAmbientVoiceSession>;

function Harness({
  apiRef,
  conversationId = "conversation-1",
  meetingState = "idle",
  settings = { ...voiceSettings, listeningEnabled: true },
  sessionRef,
  pendingRef,
}: {
  apiRef: MutableRefObject<SessionApi | null>;
  conversationId?: string | null;
  meetingState?: "idle" | "active";
  settings?: VoiceSettings | null;
  sessionRef: MutableRefObject<ConversationSession>;
  pendingRef: MutableRefObject<PendingConversationPrompt[]>;
}) {
  apiRef.current = useAmbientVoiceSession({
    selectedConversationId: conversationId,
    voiceSettings: settings,
    voicePolicy,
    meetingState,
    conversationSessionRef: sessionRef,
    pendingVoicePromptsRef: pendingRef,
    setError: () => undefined,
    setRuntimeActivity: (update) => (typeof update === "function" ? update([]) : update),
    stopSpeech: async () => undefined,
    submitPrompt: async (prompt) => {
      submitted.push(prompt);
    },
    persistListeningEnabled: async () => undefined,
  });
  return null;
}

const submitted: string[] = [];

function installAudioGlobals() {
  const previous = {
    AudioContext: globalThis.AudioContext,
    AudioWorkletNode: globalThis.AudioWorkletNode,
  };
  (globalThis as { AudioContext: unknown }).AudioContext = FakeAudioContext;
  (globalThis as { AudioWorkletNode: unknown }).AudioWorkletNode = FakeAudioWorkletNode;
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: {
      getUserMedia: async () => ({ getTracks: () => [{ stop: () => undefined }] }),
    },
  });
  return () => {
    (globalThis as { AudioContext: unknown }).AudioContext = previous.AudioContext;
    (globalThis as { AudioWorkletNode: unknown }).AudioWorkletNode = previous.AudioWorkletNode;
  };
}

describe("ambient voice session", () => {
  let root: Root | null = null;
  let restoreDom: (() => void) | null = null;
  let restoreAudio: (() => void) | null = null;

  beforeEach(() => {
    resetTauriCoreMock();
    submitted.length = 0;
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restoreAudio?.();
    restoreAudio = null;
    restoreDom?.();
    restoreDom = null;
  });

  test("derives capture settings from the conversation policy", () => {
    expect(effectiveCaptureSettings(null, null)).toBeNull();
    expect(effectiveCaptureSettings(voiceSettings, null)?.silenceTimeoutMs).toBe(1_500);
    expect(effectiveCaptureSettings(voiceSettings, voicePolicy)?.silenceTimeoutMs).toBe(1_500);
  });

  test("captures, delivers a final utterance, and suspends for a meeting", async () => {
    restoreDom = installJsdom().restore;
    restoreAudio = installAudioGlobals();
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const apiRef: MutableRefObject<SessionApi | null> = { current: null };
    const sessionRef: MutableRefObject<ConversationSession> = {
      current: { ...initialConversationSession },
    };
    const pendingRef: MutableRefObject<PendingConversationPrompt[]> = { current: [] };
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(Harness, { apiRef, sessionRef, pendingRef })));
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(true);
    });
    const start = invokeCalls.find((call) => call.command === "start_voice_asr_session");
    const sessionId =
      (start?.args as { input?: { sessionId?: string } } | undefined)?.input?.sessionId ?? "";
    expect(sessionId.length).toBeGreaterThan(0);
    const channel = channels.at(-1);
    await act(async () => {
      channel?.onmessage?.({
        type: "ready",
        sessionId,
        currentUtteranceId: "u1",
        protocol: "batch-agreement",
        scope: "all-speakers",
      });
      channel?.onmessage?.({
        type: "partial",
        sessionId,
        utteranceId: "u1",
        revision: 1,
        startMs: 0,
        endMs: 10,
        stableText: "hello",
        unstableText: "",
        language: "ja",
      });
      channel?.onmessage?.({
        type: "final",
        sessionId,
        utteranceId: "u1",
        revision: 2,
        startMs: 0,
        endMs: 20,
        text: "hello there",
        language: "ja",
      });
    });
    expect(submitted).toContain("hello there");
    await act(async () => {
      await apiRef.current!.suspendVoiceForSpeech("speech-1");
    });
    await act(async () => {
      await apiRef.current!.resumeVoiceAfterSpeech("speech-1");
    });
    await act(async () => {
      await apiRef.current!.suspendVoiceForMeeting();
    });
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(false);
    });
    expect(apiRef.current!.listeningEnabled).toBe(false);
  });
});
