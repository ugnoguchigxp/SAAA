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

  test("each finalized ASR utterance goes directly to the Qwen turn path", async () => {
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
    expect(invokeCalls.some((c) => c.command === "receive_lfm_utterance")).toBe(false);
    expect(invokeCalls.some((c) => c.command === "speak_lfm_reply")).toBe(false);
    expect(submitted).toEqual(["hello there"]);
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
    expect(submitted).toEqual(["hello there", "please reason"]);
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
    let permissionRequests = 0;
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: {
        getUserMedia: () =>
          ++permissionRequests === 1
            ? permission
            : Promise.resolve({ getTracks: () => [{ stop: () => undefined }] }),
      },
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
    expect(invokeCalls.some((call) => call.command === "start_voice_asr_session")).toBe(false);
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(false);
    });
    expect(apiRef.current!.voiceState).toBe("stopped");

    await act(async () => {
      await apiRef.current!.toggleAmbientListening();
    });
    expect(apiRef.current!.voiceState).toBe("listening");

    let lateTrackStopped = false;
    resolvePermission({
      getTracks: () => [{ stop: () => (lateTrackStopped = true) }],
    } as unknown as MediaStream);
    await act(async () => {
      await startPromise;
    });
    expect(lateTrackStopped).toBe(true);
    expect(apiRef.current!.voiceState).toBe("listening");
    expect(apiRef.current!.voiceActionInProgress).toBe(false);
  });

  test("a repeated start request does not invalidate the pending microphone request", async () => {
    restoreDom = installJsdom().restore;
    restoreAudio = installAudioGlobals();
    let resolvePermission!: (stream: MediaStream) => void;
    let permissionRequests = 0;
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: {
        getUserMedia: () => {
          if (++permissionRequests !== 1)
            return Promise.resolve({ getTracks: () => [{ stop: () => undefined }] });
          return new Promise<MediaStream>((resolve) => {
            resolvePermission = resolve;
          });
        },
      },
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
        createElement(Harness, { apiRef, sessionRef, pendingRef, settings: voiceSettings }),
      ),
    );
    let startPromise!: Promise<void>;
    await act(async () => {
      startPromise = apiRef.current!.toggleAmbientListening(true);
      await Promise.resolve();
    });
    await act(async () => {
      await apiRef.current!.toggleAmbientListening(true);
    });
    resolvePermission({ getTracks: () => [{ stop: () => undefined }] } as unknown as MediaStream);
    await act(async () => {
      await startPromise;
    });
    expect(apiRef.current!.voiceState).toBe("listening");
    expect(apiRef.current!.voiceActionInProgress).toBe(false);
  });

  test("returns to stopped and allows retry after ASR startup fails", async () => {
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
        createElement(Harness, { apiRef, sessionRef, pendingRef, settings: voiceSettings }),
      ),
    );
    let starts = 0;
    invokeImpl.handler = async (command) => {
      if (command === "start_voice_asr_session" && ++starts === 1)
        throw new Error("asr-provider-unavailable");
      return command;
    };

    await act(async () => {
      await apiRef.current!.toggleAmbientListening();
    });
    expect(apiRef.current!.voiceState).toBe("stopped");
    expect(apiRef.current!.listeningEnabled).toBe(false);

    await act(async () => {
      await apiRef.current!.toggleAmbientListening();
    });
    expect(starts).toBe(2);
    expect(apiRef.current!.voiceState).toBe("listening");
  });

  test("records a microphone permission failure before ASR starts", async () => {
    restoreDom = installJsdom().restore;
    restoreAudio = installAudioGlobals();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: {
        getUserMedia: async () => {
          throw new DOMException("denied", "NotAllowedError");
        },
      },
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
        createElement(Harness, { apiRef, sessionRef, pendingRef, settings: voiceSettings }),
      ),
    );
    await act(async () => {
      await apiRef.current!.toggleAmbientListening();
    });
    expect(apiRef.current!.voiceState).toBe("stopped");
    const auditNames = invokeCalls
      .filter((call) => call.command === "record_frontend_audit_event")
      .map((call) => (call.args as { input: { eventName: string } }).input.eventName);
    expect(auditNames).toContain("capture-toggle-requested");
    expect(auditNames).toContain("capture-preflight-failed");
    expect(invokeCalls.some((call) => call.command === "start_voice_asr_session")).toBe(false);
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

  test("does not invoke the dormant LFM frontend for voice utterances", async () => {
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
    expect(submitted).toEqual([
      "おはよう。",
      "よう、ミュージさん、今日は何かお手伝いできることはありますか？",
    ]);
    expect(invokeCalls.some((call) => call.command === "receive_lfm_utterance")).toBe(false);
    expect(invokeCalls.some((call) => call.command === "speak_lfm_reply")).toBe(false);
  });
});
