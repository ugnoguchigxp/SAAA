import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../src/lib/contracts";
import {
  initialConversationSession,
  type ConversationSession,
  type PendingConversationPrompt,
} from "../src/lib/conversationSession";
import { channels, invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
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

  test("ASR goes only to LFM; LFM responds while Qwen runs and requests reasoning once", async () => {
    invokeImpl.handler = async (command, args) => {
      if (command === "receive_lfm_utterance") {
        const input = args as { text: string };
        return input.text === "hello there"
          ? { reasoningRequestId: null, requestContent: null, speechEpoch: 0 }
          : {
              reasoningRequestId: "lfm_reasoning_1",
              requestContent: "hello there\nplease reason",
              speechEpoch: 0,
            };
      }
      return command;
    };
    restoreDom = installJsdom().restore;
    restoreAudio = installAudioGlobals();
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const apiRef: MutableRefObject<SessionApi | null> = { current: null };
    const sessionRef: MutableRefObject<ConversationSession> = {
      current: { ...initialConversationSession, runId: "qwen-already-running" },
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
    expect(invokeCalls.some((c) => c.command === "receive_lfm_utterance")).toBe(true);
    expect(submitted).toEqual([]);
    expect(invokeCalls.some((c) => c.command === "cancel_run")).toBe(false);
    await act(async () => {
      channel?.onmessage?.({
        type: "final",
        sessionId,
        utteranceId: "u2",
        revision: 1,
        startMs: 21,
        endMs: 40,
        text: "please reason",
        language: "ja",
      });
    });
    expect(submitted).toEqual(["hello there\nplease reason"]);
    expect(invokeCalls.some((c) => c.command === "cancel_run")).toBe(false);
    await act(async () => {
      await apiRef.current!.suspendVoiceForSpeech("speech-1");
    });
    await act(async () => {
      await apiRef.current!.resumeVoiceAfterSpeech("speech-1");
    });
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(false);
    });
    expect(apiRef.current!.listeningEnabled).toBe(false);
  });

  test("keeps preparing until a pending microphone start is released", async () => {
    restoreDom = installJsdom().restore;
    restoreAudio = installAudioGlobals();
    let resolvePermission!: (stream: MediaStream) => void;
    const permission = new Promise<MediaStream>((resolve) => {
      resolvePermission = resolve;
    });
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: () => permission },
    });
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const apiRef: MutableRefObject<SessionApi | null> = { current: null };
    const sessionRef: MutableRefObject<ConversationSession> = {
      current: { ...initialConversationSession },
    };
    const pendingRef: MutableRefObject<PendingConversationPrompt[]> = { current: [] };
    root = createRoot(document.getElementById("root")!);
    await act(async () =>
      root!.render(
        createElement(Harness, {
          apiRef,
          sessionRef,
          pendingRef,
          settings: voiceSettings,
        }),
      ),
    );

    let startPromise!: Promise<void>;
    await act(async () => {
      startPromise = apiRef.current!.toggleAmbientListening(true);
      await Promise.resolve();
    });
    expect(apiRef.current!.voiceState).toBe("preparing");
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(false);
    });
    expect(apiRef.current!.voiceState).toBe("preparing");

    let lateTrackStopped = false;
    resolvePermission({
      getTracks: () => [{ stop: () => (lateTrackStopped = true) }],
    } as unknown as MediaStream);
    await act(async () => {
      await startPromise;
    });
    expect(lateTrackStopped).toBe(true);
    expect(apiRef.current!.voiceState).toBe("stopped");
    expect(apiRef.current!.voiceActionInProgress).toBe(false);
  });

  test("releases capture and returns to stopped when ASR stop fails", async () => {
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
    await act(async () =>
      root!.render(
        createElement(Harness, {
          apiRef,
          sessionRef,
          pendingRef,
          settings: voiceSettings,
        }),
      ),
    );
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(true);
    });
    expect(apiRef.current!.voiceState).toBe("listening");

    invokeImpl.handler = async (command) => {
      if (command === "stop_voice_asr_session") throw new Error("stop failed");
      return command;
    };
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(false);
    });
    expect(apiRef.current!.voiceState).toBe("stopped");
    expect(apiRef.current!.voiceBusy).toBe(false);
  });

  test("pauses capture while LFM playback is audible and drops self-speech", async () => {
    invokeImpl.handler = async (command, args) => {
      if (command === "receive_lfm_utterance") {
        const input = args as { text?: string };
        if (input.text?.includes("お手伝い")) {
          return {
            reasoningRequestId: null,
            requestContent: null,
            speechEpoch: 1,
            ignoredAsSelfSpeech: true,
          };
        }
        return {
          reasoningRequestId: null,
          requestContent: null,
          speechEpoch: 1,
          ignoredAsSelfSpeech: false,
        };
      }
      return command;
    };
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
    const channel = channels.at(-1);
    await act(async () => {
      channel?.onmessage?.({
        type: "ready",
        sessionId,
        currentUtteranceId: "u-hello",
        protocol: "batch-agreement",
        scope: "all-speakers",
      });
      channel?.onmessage?.({
        type: "final",
        sessionId,
        utteranceId: "u-hello",
        revision: 1,
        startMs: 0,
        endMs: 20,
        text: "おはよう。",
        language: "ja",
      });
    });
    const speech = invokeCalls.find((call) => call.command === "speak_lfm_reply");
    const onEvent = (speech?.args as { onEvent?: { onmessage: ((event: unknown) => void) | null } })
      .onEvent;
    await act(async () => {
      onEvent?.onmessage?.({ type: "speechStarted", runId: "lfm-speech-1" });
    });
    expect(invokeCalls.some((call) => call.command === "stop_voice_asr_session")).toBe(true);
    const spoken = invokeCalls.filter((call) => call.command === "speak_lfm_reply").length;
    await act(async () => {
      channel?.onmessage?.({
        type: "final",
        sessionId,
        utteranceId: "u-echo",
        revision: 1,
        startMs: 30,
        endMs: 80,
        text: "よう、ミュージさん、今日は何かお手伝いできることはありますか？",
        language: "ja",
      });
    });
    expect(invokeCalls.filter((call) => call.command === "speak_lfm_reply").length).toBe(spoken);
  });
});
