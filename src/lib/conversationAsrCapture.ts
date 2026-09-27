import { stageAudioUpload } from "./audioIpc";
import { acquireAudioCapture } from "./audioCaptureCoordinator";
import {
  audioBackendStatus,
  nativeCapturePreferred,
  startNativeVoiceCapture,
  stopNativeVoiceCapture,
} from "./audioBackend";
import { startBrowserVoiceCapture, type BrowserVoiceCapture } from "./browserVoiceCapture";
import { microphoneErrorMessage } from "./microphone";
import { createConversationCaptureAudit } from "./conversationCaptureAudit";
import {
  state,
  publish,
  updateEntry,
  conversationAsrSnapshot,
  failPendingQwenEntries,
} from "./conversationAsrState";
export { conversationAsrSnapshot, subscribeConversationAsr } from "./conversationAsrState";
export type { CaptureEntry } from "./conversationAsrState";
import { releaseConversationAsrSession, transcribeConversationAudio } from "./runtime";
import { VoiceActivityDetector } from "./voiceActivity";
import { VoiceAsrPacketizer } from "../features/voice/voiceAsrPacketizer";
import {
  appendQwenAsrAudio,
  conversationAsrTransport,
  startQwenAsrSession,
  stopQwenAsrSession,
  type QwenRealtimeAsrEvent,
} from "./qwenRealtimeAsr";

const SAMPLE_RATE = 16_000;
const PARTIAL_SAMPLES = SAMPLE_RATE * 1.5;
const PREROLL_SAMPLES = SAMPLE_RATE / 2;
const MAX_UTTERANCE_SAMPLES = SAMPLE_RATE * 30;
const MAX_PENDING_FINALS = 8;

type RecognitionJob = {
  kind: "partial" | "final";
  id: string;
  samples: Float32Array;
};

let capture: BrowserVoiceCapture | null = null;
let captureAecActive = false;
let nativePlaybackActive = false;
let playbackActive = false;
let detector: VoiceActivityDetector | null = null;
let threshold = 0.008;
let silenceTimeoutMs = 1_500;
let preroll: Float32Array[] = [];
let prerollCount = 0;
let utterance: Float32Array[] = [];
let utteranceCount = 0;
let currentUtteranceId: string | null = null;
let lastPartialSamples = 0;
let pendingPartial: RecognitionJob | null = null;
const pendingFinals: RecognitionJob[] = [];
let drainingFinals = false;
let drainingPartials = false;
let finalCompletion: Promise<void> = Promise.resolve();
let partialCompletion: Promise<void> = Promise.resolve();
let captureSessionId: string | null = null;
let recognitionSequence = 0;
let recognitionError: { sequence: number; message: string } | null = null;
const auditCapture = createConversationCaptureAudit(() => captureSessionId);
let activeTransport: "http" | "qwen-realtime" = "http";
let packetizer: VoiceAsrPacketizer | null = null;
let audioSendTail: Promise<void> = Promise.resolve();
let pendingAudioPackets = 0;
const earlyFinals = new Map<string, { text: string; language: string | null }>();
const earlyFailures = new Map<string, string>();
const partialAfterFinal = new Set<string>();

function sendLiveFrame(id: string, frame: Float32Array) {
  if (!packetizer || !captureSessionId) return;
  const packets = packetizer.append(frame);
  for (let index = 0; index < packets.length; index += 1) {
    const packet = packets[index]!;
    if (pendingAudioPackets >= 10) {
      for (const remaining of packets.slice(index)) remaining.fill(0);
      publish({ error: "ASRへの音声送信が滞留しています。" });
      void stopConversationAsr("ASRへの音声送信が滞留しています。");
      return;
    }
    pendingAudioPackets += 1;
    const sessionId = captureSessionId;
    audioSendTail = audioSendTail
      .then(() => appendQwenAsrAudio(sessionId, id, packet))
      .catch((cause) => {
        publish({ error: String(cause) });
        void stopConversationAsr(String(cause));
      })
      .finally(() => {
        packet.fill(0);
        pendingAudioPackets -= 1;
      });
  }
}

function flushLiveFrame(id: string) {
  const packet = packetizer?.flushPadded();
  if (!captureSessionId) return;
  const sessionId = captureSessionId;
  if (packet) {
    pendingAudioPackets += 1;
    audioSendTail = audioSendTail
      .then(() => appendQwenAsrAudio(sessionId, id, packet))
      .catch((cause) => publish({ error: String(cause) }))
      .finally(() => {
        packet.fill(0);
        pendingAudioPackets -= 1;
      });
  }
  // Both VADs use the configured silence duration. Allow a short server tail
  // for detection differences without requiring another microphone utterance.
  for (let index = 0; index < 5; index += 1) sendLiveFrame(id, new Float32Array(1_600));
}

function onQwenAsrEvent(event: QwenRealtimeAsrEvent) {
  if (event.sessionId !== captureSessionId) return;
  if (event.type === "speechStarted") {
    if (event.utteranceId === currentUtteranceId && earlyFinals.has(event.utteranceId))
      partialAfterFinal.add(event.utteranceId);
  } else if (event.type === "partial") {
    if (event.utteranceId === currentUtteranceId && !state.playbackLimited) {
      const completedPrefix = earlyFinals.get(event.utteranceId)?.text ?? "";
      if (completedPrefix) partialAfterFinal.add(event.utteranceId);
      publish({ interimText: completedPrefix + event.text });
    }
  } else if (event.type === "final") {
    const entry = state.entries.find((candidate) => candidate.id === event.utteranceId);
    if (entry?.status === "queued") {
      const early = earlyFinals.get(event.utteranceId);
      const prefix = early?.text ?? "";
      earlyFinals.delete(event.utteranceId);
      partialAfterFinal.delete(event.utteranceId);
      updateEntry(event.utteranceId, {
        status: "completed",
        text: `${prefix}${event.text}`.trim() || null,
        language: event.language ?? early?.language ?? null,
        provider: "Qwen ASR Realtime",
      });
    } else if (event.utteranceId && event.utteranceId === currentUtteranceId) {
      const previous = earlyFinals.get(event.utteranceId);
      earlyFinals.set(event.utteranceId, {
        text: `${previous?.text ?? ""}${event.text}`,
        language: event.language ?? previous?.language ?? null,
      });
      partialAfterFinal.delete(event.utteranceId);
    }
  } else if (event.type === "failed") {
    const entry = state.entries.find((candidate) => candidate.id === event.utteranceId);
    if (entry) {
      if (entry.status === "queued")
        updateEntry(event.utteranceId!, { status: "failed", error: event.message });
    } else if (event.utteranceId && event.utteranceId === currentUtteranceId)
      earlyFailures.set(event.utteranceId, event.message);
    else {
      publish({ error: event.message });
      if (!event.utteranceId) void stopConversationAsr(event.message);
    }
  } else if (event.type === "stopped") {
    failPendingQwenEntries();
    if (state.phase === "recording" || state.phase === "starting")
      void stopConversationAsr("Qwen ASR接続が終了しました。");
  }
}

export function queueConversationAsrDelivery(id: string): boolean {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (!entry || entry.status !== "completed" || !entry.text?.trim() || entry.deliveryQueued)
    return false;
  updateEntry(id, { deliveryQueued: true });
  auditCapture("conversation-asr-delivery-queued", "decision", id, {
    textBytes: entry.text.length,
  });
  return true;
}

export function failConversationAsrDelivery(id: string, error: string) {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (entry?.deliveryQueued && entry.provider)
    updateEntry(id, { status: "failed", deliveryQueued: false, error });
}

export function retryConversationAsrDelivery(id: string): string | null {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (
    !entry ||
    entry.status !== "failed" ||
    !entry.provider ||
    !entry.text?.trim() ||
    entry.deliveryQueued
  )
    return null;
  updateEntry(id, { status: "completed", deliveryQueued: true, error: null });
  return entry.text;
}

function newDetector() {
  return new VoiceActivityDetector({
    sampleRate: SAMPLE_RATE,
    speechThresholdRms: threshold,
    silenceTimeoutMs,
  });
}

function updatePlaybackLimit() {
  // Playback is observational state. AEC runs in the native capture path;
  // microphone frames must remain available for speech during TTS.
  if (state.playbackLimited) publish({ playbackLimited: false });
}

function clearPreroll() {
  for (const frame of preroll) frame.fill(0);
  preroll = [];
  prerollCount = 0;
}

function clearUtterance() {
  for (const frame of utterance) frame.fill(0);
  utterance = [];
  utteranceCount = 0;
  currentUtteranceId = null;
  lastPartialSamples = 0;
  detector = newDetector();
  publish({ interimText: "", speechDetected: false });
}

function samplesOf(frames: Float32Array[], count: number) {
  const samples = new Float32Array(count);
  let offset = 0;
  for (const frame of frames) {
    samples.set(frame, offset);
    offset += frame.length;
  }
  return samples;
}

function enqueue(job: RecognitionJob) {
  if (job.kind === "partial") {
    if (pendingPartial)
      auditCapture("conversation-asr-partial-replaced", "decision", pendingPartial.id, {
        samples: pendingPartial.samples.length,
      });
    pendingPartial?.samples.fill(0);
    pendingPartial = job;
    if (!drainingPartials) partialCompletion = drainPartials();
  } else if (pendingFinals.length < MAX_PENDING_FINALS) {
    pendingFinals.push(job);
    if (!drainingFinals) finalCompletion = drainFinals();
  } else {
    job.samples.fill(0);
    updateEntry(job.id, { status: "failed", error: "ASRの確定処理が滞留しています。" });
    auditCapture(
      "conversation-asr-final-rejected",
      "error",
      job.id,
      { reason: "queue-full" },
      "failure",
    );
  }
}

async function drainFinals() {
  drainingFinals = true;
  try {
    while (pendingFinals.length) {
      const job = pendingFinals.shift()!;
      updateEntry(job.id, { status: "transcribing" });
      await processJob(job);
    }
  } finally {
    drainingFinals = false;
  }
}

async function drainPartials() {
  drainingPartials = true;
  try {
    while (pendingPartial) {
      const job = pendingPartial;
      pendingPartial = null;
      await processJob(job);
    }
  } finally {
    drainingPartials = false;
  }
}

async function processJob(job: RecognitionJob) {
  const sequence = ++recognitionSequence;
  try {
    auditCapture("conversation-asr-upload", "start", job.id, {
      kind: job.kind,
      samples: job.samples.length,
    });
    const uploadId = await stageAudioUpload(job.samples, "conversation-asr");
    auditCapture(
      "conversation-asr-upload",
      "terminal",
      job.id,
      { kind: job.kind, audioUploadId: uploadId },
      "success",
    );
    job.samples.fill(0);
    const result = await transcribeConversationAudio(uploadId, job.id, job.kind);
    if (recognitionError && sequence >= recognitionError.sequence) {
      if (state.error === recognitionError.message) publish({ error: null });
      recognitionError = null;
    }
    auditCapture(
      "conversation-asr-ipc-result",
      "terminal",
      job.id,
      { kind: job.kind, textBytes: result.text.length },
      "success",
    );
    if (job.kind === "final") {
      updateEntry(job.id, {
        status: "completed",
        text: result.text.trim() || null,
        language: result.language,
        provider: result.providerLabel,
      });
    } else if (currentUtteranceId === job.id && !state.playbackLimited) {
      publish({ interimText: result.text });
    } else if (job.kind === "partial") {
      auditCapture("conversation-asr-partial-ignored", "decision", job.id, {
        reason: state.playbackLimited ? "playback-limited" : "utterance-finalized",
      });
    }
  } catch (cause) {
    const message = String(cause);
    auditCapture(
      "conversation-asr-job-failed",
      "error",
      job.id,
      { kind: job.kind, error: message },
      "failure",
    );
    if (job.kind === "final") {
      updateEntry(
        job.id,
        message.includes("ASR_NO_SPEECH")
          ? { status: "completed", text: null }
          : { status: "failed", error: message },
      );
      if (message.includes("ASR_NO_SPEECH"))
        auditCapture("conversation-asr-no-speech", "decision", job.id, {}, "degraded");
    } else if (!message.includes("ASR_NO_SPEECH") && currentUtteranceId === job.id) {
      recognitionError = { sequence, message };
      publish({ error: message });
    }
  } finally {
    job.samples.fill(0);
  }
}

function finalizeUtterance() {
  const id = currentUtteranceId;
  if (!id || utteranceCount < SAMPLE_RATE / 10) {
    clearUtterance();
    return;
  }
  const seconds = utteranceCount / SAMPLE_RATE;
  auditCapture("conversation-asr-utterance-finalized", "decision", id, {
    seconds,
    samples: utteranceCount,
  });
  const samples = activeTransport === "http" ? samplesOf(utterance, utteranceCount) : null;
  if (activeTransport === "qwen-realtime") flushLiveFrame(id);
  const early = earlyFinals.get(id);
  const waitingForFinal = Boolean(early && partialAfterFinal.has(id));
  if (!waitingForFinal) earlyFinals.delete(id);
  partialAfterFinal.delete(id);
  const earlyFailure = earlyFailures.get(id);
  earlyFailures.delete(id);
  publish({
    entries: [
      {
        id,
        recordedAt: new Date().toLocaleString(),
        seconds,
        status: early && !waitingForFinal ? "completed" : earlyFailure ? "failed" : "queued",
        text: early?.text.trim() || state.interimText || null,
        language: early?.language ?? null,
        provider: activeTransport === "qwen-realtime" ? "Qwen ASR Realtime" : null,
        error: earlyFailure ?? null,
        deliveryQueued: false,
      },
      ...state.entries,
    ],
  });
  if (pendingPartial?.id === id) {
    auditCapture("conversation-asr-partial-cancelled", "decision", id, { reason: "finalized" });
    pendingPartial.samples.fill(0);
    pendingPartial = null;
  }
  clearUtterance();
  if (samples) enqueue({ kind: "final", id, samples });
}

function addFrame(frame: Float32Array) {
  if (
    (state.phase !== "recording" && state.phase !== "stopping") ||
    state.playbackLimited ||
    !detector
  )
    return;
  const observation = detector.observe(frame);
  if (!currentUtteranceId) {
    preroll.push(frame.slice());
    prerollCount += frame.length;
    while (prerollCount > PREROLL_SAMPLES + frame.length && preroll.length > 1) {
      const old = preroll.shift()!;
      prerollCount -= old.length;
      old.fill(0);
    }
    if (!observation.hasSpeech) return;
    currentUtteranceId = crypto.randomUUID();
    auditCapture("conversation-asr-speech-detected", "start", currentUtteranceId, {
      prerollSamples: prerollCount,
    });
    const recorded = preroll;
    utterance = activeTransport === "http" ? recorded : [];
    utteranceCount = prerollCount;
    preroll = [];
    prerollCount = 0;
    if (activeTransport === "qwen-realtime") {
      for (const frame of recorded) {
        sendLiveFrame(currentUtteranceId, frame);
        frame.fill(0);
      }
    }
    publish({ speechDetected: true, interimText: "" });
  } else {
    if (activeTransport === "http") utterance.push(frame.slice());
    utteranceCount += frame.length;
    if (activeTransport === "qwen-realtime") sendLiveFrame(currentUtteranceId, frame);
  }
  if (
    observation.shouldFinalize ||
    (activeTransport === "http" && utteranceCount >= MAX_UTTERANCE_SAMPLES)
  ) {
    finalizeUtterance();
  } else if (activeTransport === "http" && utteranceCount - lastPartialSamples >= PARTIAL_SAMPLES) {
    lastPartialSamples = utteranceCount;
    auditCapture("conversation-asr-partial-queued", "progress", currentUtteranceId, {
      samples: utteranceCount,
    });
    enqueue({
      kind: "partial",
      id: currentUtteranceId!,
      samples: samplesOf(utterance, utteranceCount),
    });
  }
}

export function setConversationAsrPlaybackActive(active: boolean, inputId?: string) {
  if (playbackActive === active) return;
  playbackActive = active;
  updatePlaybackLimit();
  auditCapture("conversation-asr-playback-limit", "state", inputId ?? captureSessionId, {
    active,
    playbackLimited: state.playbackLimited,
    captureAecActive,
    nativePlaybackActive,
  });
}

export async function startConversationAsr(
  inputDeviceId: string,
  echoCancellation: boolean,
  vadSensitivity: "low" | "medium" | "high" = "medium",
  configuredSilenceTimeoutMs = 1_500,
) {
  if (state.phase !== "idle") return;
  const sessionId = crypto.randomUUID();
  captureSessionId = sessionId;
  auditCapture("conversation-asr-capture-start", "request", captureSessionId, {
    inputDeviceId,
    echoCancellation,
    vadSensitivity,
    silenceTimeoutMs: configuredSilenceTimeoutMs,
  });
  threshold = vadSensitivity === "high" ? 0.006 : vadSensitivity === "low" ? 0.012 : 0.008;
  silenceTimeoutMs = configuredSilenceTimeoutMs;
  captureAecActive = false;
  nativePlaybackActive = false;
  detector = newDetector();
  recognitionError = null;
  publish({ phase: "starting", error: null });
  try {
    activeTransport = await conversationAsrTransport();
    if (conversationAsrSnapshot().phase !== "starting") {
      activeTransport = "http";
      return;
    }
    if (activeTransport === "qwen-realtime") {
      packetizer = new VoiceAsrPacketizer();
      audioSendTail = Promise.resolve();
      pendingAudioPackets = 0;
      await startQwenAsrSession(sessionId, onQwenAsrEvent, silenceTimeoutMs);
      if (conversationAsrSnapshot().phase !== "starting") {
        await stopQwenAsrSession(sessionId).catch(() => undefined);
        packetizer?.reset();
        packetizer = null;
        activeTransport = "http";
        return;
      }
    }
    let started: BrowserVoiceCapture | null = null;
    let backend = "browser";
    const native = await audioBackendStatus().catch(() => null);
    if (native && nativeCapturePreferred(native, echoCancellation)) {
      const release = acquireAudioCapture("chat");
      try {
        const status = await startNativeVoiceCapture(
          addFrame,
          (reason) => void stopConversationAsr(reason),
          (current) => {
            captureAecActive = current.aecActive;
            nativePlaybackActive = current.playbackActive;
            updatePlaybackLimit();
          },
        );
        captureAecActive = status.aecActive;
        nativePlaybackActive = status.playbackActive;
        started = {
          stop: async () => {
            try {
              await stopNativeVoiceCapture();
            } finally {
              release();
            }
          },
        };
        backend = "native";
      } catch (cause) {
        captureAecActive = false;
        nativePlaybackActive = false;
        auditCapture(
          "conversation-asr-native-fallback",
          "decision",
          captureSessionId,
          { error: String(cause) },
          "degraded",
        );
        await stopNativeVoiceCapture().catch(() => undefined);
        release();
      }
    }
    const browserEchoCancellation = echoCancellation && !native?.macosMajor;
    if (!started && echoCancellation && !browserEchoCancellation) {
      auditCapture(
        "conversation-asr-browser-fallback",
        "decision",
        captureSessionId,
        { echoCancellation: false },
        "degraded",
      );
    }
    started ??= await startBrowserVoiceCapture(
      addFrame,
      (reason) => void stopConversationAsr(reason),
      inputDeviceId,
      browserEchoCancellation,
    );
    if (conversationAsrSnapshot().phase !== "starting") {
      await started.stop();
      if (activeTransport === "qwen-realtime")
        await stopQwenAsrSession(sessionId).catch(() => undefined);
      packetizer?.reset();
      packetizer = null;
      activeTransport = "http";
      captureAecActive = false;
      nativePlaybackActive = false;
      updatePlaybackLimit();
      return;
    }
    capture = started;
    publish({ phase: "recording" });
    updatePlaybackLimit();
    auditCapture(
      "conversation-asr-capture-start",
      "terminal",
      captureSessionId,
      { backend, captureAecActive },
      "success",
    );
  } catch (cause) {
    if (activeTransport === "qwen-realtime" && captureSessionId)
      await stopQwenAsrSession(captureSessionId).catch(() => undefined);
    packetizer?.reset();
    packetizer = null;
    activeTransport = "http";
    captureAecActive = false;
    nativePlaybackActive = false;
    clearPreroll();
    clearUtterance();
    const error = microphoneErrorMessage(cause);
    publish({ phase: "idle", error });
    auditCapture("conversation-asr-capture-start", "error", captureSessionId, { error }, "failure");
  }
}

export async function stopConversationAsr(reason?: string) {
  if (state.phase !== "recording" && state.phase !== "starting") return;
  auditCapture("conversation-asr-capture-stop", "request", captureSessionId, {
    reason: reason ?? "user",
  });
  publish({ phase: "stopping" });
  const current = capture;
  capture = null;
  captureAecActive = false;
  nativePlaybackActive = false;
  updatePlaybackLimit();
  let error = reason ?? null;
  try {
    await current?.stop();
  } catch (cause) {
    error = [error, String(cause)].filter(Boolean).join(" · ");
  }
  if (currentUtteranceId) finalizeUtterance();
  clearPreroll();
  if (activeTransport === "qwen-realtime") {
    await audioSendTail;
    if (captureSessionId)
      await stopQwenAsrSession(captureSessionId).catch((cause) =>
        publish({ error: String(cause) }),
      );
    failPendingQwenEntries();
    packetizer?.reset();
    packetizer = null;
  } else {
    await Promise.all([finalCompletion, partialCompletion]);
    auditCapture("conversation-asr-session-release", "request", captureSessionId);
    await releaseConversationAsrSession()
      .then(() =>
        auditCapture(
          "conversation-asr-session-release",
          "terminal",
          captureSessionId,
          {},
          "success",
        ),
      )
      .catch((cause) => {
        const releaseError = String(cause);
        publish({ error: releaseError });
        auditCapture(
          "conversation-asr-session-release",
          "error",
          captureSessionId,
          { error: releaseError },
          "failure",
        );
      });
  }
  activeTransport = "http";
  earlyFinals.clear();
  earlyFailures.clear();
  partialAfterFinal.clear();
  publish({ phase: "idle", error: error ?? state.error });
  auditCapture(
    "conversation-asr-capture-stop",
    error ? "error" : "terminal",
    captureSessionId,
    error ? { error } : {},
    error ? "failure" : "success",
  );
  captureSessionId = null;
}
