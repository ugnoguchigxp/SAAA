import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../src/lib/contracts";

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

export const voiceSettings: VoiceSettings = {
  listeningEnabled: false,
  inputDeviceId: "default",
  outputDeviceId: "default",
  vadSensitivity: "medium",
  silenceTimeoutMs: 1_500,
  allowedLanguages: ["ja"],
  autoSpeak: true,
  aecEnabled: true,
  otherAudioDucking: "min" as const,
  vpioOnBluetooth: false,
  bargeInEnabled: true,
};

export const voicePolicy: ConversationVoicePolicySnapshot = {
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

export function installAudioGlobals() {
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
