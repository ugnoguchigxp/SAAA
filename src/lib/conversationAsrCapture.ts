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
import { recordConversationCaptureAuditEvent } from "./auditRuntime";
import { releaseConversationAsrSession, transcribeConversationAudio } from "./runtime";
import { VoiceActivityDetector } from "./voiceActivity";

const SAMPLE_RATE = 16_000;
const PARTIAL_SAMPLES = SAMPLE_RATE * 1.5;
const PREROLL_SAMPLES = SAMPLE_RATE / 2;
const MAX_UTTERANCE_SAMPLES = SAMPLE_RATE * 30;
const MAX_PENDING_FINALS = 8;

export type CaptureEntry = {
  id: string;
  recordedAt: string;
  seconds: number;
  status: "queued" | "transcribing" | "completed" | "failed";
  text: string | null;
  language: string | null;
  provider: string | null;
  error: string | null;
  deliveryQueued: boolean;
};

type CaptureState = {
  phase: "idle" | "starting" | "recording" | "stopping";
  error: string | null;
  entries: CaptureEntry[];
  interimText: string;
  speechDetected: boolean;
  playbackLimited: boolean;
};

type RecognitionJob = {
  kind: "partial" | "final";
  id: string;
  samples: Float32Array;
};

let state: CaptureState = {
  phase: "idle",
  error: null,
  entries: [],
  interimText: "",
  speechDetected: false,
  playbackLimited: false,
};
const listeners = new Set<() => void>();
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
let draining = false;
let drainCompletion: Promise<void> = Promise.resolve();
let captureSessionId: string | null = null;
let captureAuditSequence = 0;

function auditCapture(eventName: string, phase: "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error", correlationId: string | null, attributes: Record<string, string | number | boolean> = {}, outcome?: "success" | "failure" | "degraded") {
  const sequence = ++captureAuditSequence;
  const observedAtMs = Date.now();
  attributes = { ...attributes, sequence, observedAtMs, captureSessionId: captureSessionId ?? "" };
  const serialized = JSON.stringify(attributes);
  if (new TextEncoder().encode(serialized).length > 1_500) {
    const characters = [...serialized];
    const parts = Math.ceil(characters.length / 180);
    for (let index = 0; index < parts; index += 1) {
      recordConversationCaptureAuditEvent({ eventName: `${eventName}-detail`, phase: "progress", correlationId, attributes: { sequence, observedAtMs, part: index + 1, parts, text: characters.slice(index * 180, (index + 1) * 180).join("") } });
    }
    attributes = { sequence, observedAtMs, overflow: true, bytes: new TextEncoder().encode(serialized).length };
  }
  recordConversationCaptureAuditEvent({ eventName, phase, outcome, correlationId, attributes });
}

export function subscribeConversationAsr(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function conversationAsrSnapshot() {
  return state;
}

function publish(change: Partial<CaptureState>) {
  state = { ...state, ...change };
  listeners.forEach((listener) => listener());
}

function updateEntry(id: string, change: Partial<CaptureEntry>) {
  publish({
    entries: state.entries.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)),
  });
}

export function queueConversationAsrDelivery(id: string): boolean {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (!entry || entry.status !== "completed" || !entry.text?.trim() || entry.deliveryQueued)
    return false;
  updateEntry(id, { deliveryQueued: true });
  auditCapture("conversation-asr-delivery-queued", "decision", id, { textBytes: entry.text.length });
  return true;
}

function newDetector() {
  return new VoiceActivityDetector({
    sampleRate: SAMPLE_RATE,
    speechThresholdRms: threshold,
    silenceTimeoutMs,
  });
}

function updatePlaybackLimit() {
  const limited = playbackActive && !(captureAecActive && nativePlaybackActive);
  if (state.playbackLimited !== limited) publish({ playbackLimited: limited });
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
    if (pendingPartial) auditCapture("conversation-asr-partial-replaced", "decision", pendingPartial.id, { samples: pendingPartial.samples.length });
    pendingPartial?.samples.fill(0);
    pendingPartial = job;
  } else if (pendingFinals.length < MAX_PENDING_FINALS) {
    pendingFinals.push(job);
  } else {
    job.samples.fill(0);
    updateEntry(job.id, { status: "failed", error: "ASRの確定処理が滞留しています。" });
    auditCapture("conversation-asr-final-rejected", "error", job.id, { reason: "queue-full" }, "failure");
  }
  if (!draining) drainCompletion = drainJobs();
}

async function drainJobs() {
  draining = true;
  try {
    while (pendingFinals.length || pendingPartial) {
      const job = pendingFinals.shift() ?? pendingPartial!;
      if (job === pendingPartial) pendingPartial = null;
      if (job.kind === "final") updateEntry(job.id, { status: "transcribing" });
      try {
        auditCapture("conversation-asr-upload", "start", job.id, { kind: job.kind, samples: job.samples.length });
        const uploadId = await stageAudioUpload(job.samples, "conversation-asr");
        auditCapture("conversation-asr-upload", "terminal", job.id, { kind: job.kind, audioUploadId: uploadId }, "success");
        job.samples.fill(0);
        const result = await transcribeConversationAudio(uploadId, job.id, job.kind);
        auditCapture("conversation-asr-ipc-result", "terminal", job.id, { kind: job.kind, textBytes: result.text.length }, "success");
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
          auditCapture("conversation-asr-partial-ignored", "decision", job.id, { reason: state.playbackLimited ? "playback-limited" : "utterance-finalized" });
        }
      } catch (cause) {
        const message = String(cause);
        auditCapture("conversation-asr-job-failed", "error", job.id, { kind: job.kind, error: message }, "failure");
        if (job.kind === "final") {
          updateEntry(
            job.id,
            message.includes("ASR_NO_SPEECH")
              ? { status: "completed", text: null }
              : { status: "failed", error: message },
          );
          if (message.includes("ASR_NO_SPEECH")) auditCapture("conversation-asr-no-speech", "decision", job.id, {}, "degraded");
        } else if (!message.includes("ASR_NO_SPEECH") && currentUtteranceId === job.id) {
          publish({ error: message });
        }
      } finally {
        job.samples.fill(0);
      }
    }
  } finally {
    draining = false;
  }
}

function finalizeUtterance() {
  const id = currentUtteranceId;
  if (!id || utteranceCount < SAMPLE_RATE / 10) {
    clearUtterance();
    return;
  }
  const seconds = utteranceCount / SAMPLE_RATE;
  auditCapture("conversation-asr-utterance-finalized", "decision", id, { seconds, samples: utteranceCount });
  const samples = samplesOf(utterance, utteranceCount);
  publish({
    entries: [
      {
        id,
        recordedAt: new Date().toLocaleString(),
        seconds,
        status: "queued",
        text: state.interimText || null,
        language: null,
        provider: null,
        error: null,
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
  enqueue({ kind: "final", id, samples });
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
    auditCapture("conversation-asr-speech-detected", "start", currentUtteranceId, { prerollSamples: prerollCount });
    utterance = preroll;
    utteranceCount = prerollCount;
    preroll = [];
    prerollCount = 0;
    publish({ speechDetected: true, interimText: "" });
  } else {
    utterance.push(frame.slice());
    utteranceCount += frame.length;
  }
  if (observation.shouldFinalize || utteranceCount >= MAX_UTTERANCE_SAMPLES) {
    finalizeUtterance();
  } else if (utteranceCount - lastPartialSamples >= PARTIAL_SAMPLES) {
    lastPartialSamples = utteranceCount;
    auditCapture("conversation-asr-partial-queued", "progress", currentUtteranceId, { samples: utteranceCount });
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
  if (active) {
    if (currentUtteranceId) finalizeUtterance();
    clearPreroll();
    clearUtterance();
    pendingPartial?.samples.fill(0);
    pendingPartial = null;
  }
  updatePlaybackLimit();
  auditCapture("conversation-asr-playback-limit", "state", inputId ?? captureSessionId, { active, playbackLimited: state.playbackLimited, captureAecActive, nativePlaybackActive });
}

export async function startConversationAsr(
  inputDeviceId: string,
  echoCancellation: boolean,
  vadSensitivity: "low" | "medium" | "high" = "medium",
  configuredSilenceTimeoutMs = 1_500,
) {
  if (state.phase !== "idle") return;
  captureSessionId = crypto.randomUUID();
  auditCapture("conversation-asr-capture-start", "request", captureSessionId, { inputDeviceId, echoCancellation, vadSensitivity, silenceTimeoutMs: configuredSilenceTimeoutMs });
  threshold = vadSensitivity === "high" ? 0.006 : vadSensitivity === "low" ? 0.012 : 0.008;
  silenceTimeoutMs = configuredSilenceTimeoutMs;
  captureAecActive = false;
  nativePlaybackActive = false;
  detector = newDetector();
  publish({ phase: "starting", error: null });
  try {
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
        auditCapture("conversation-asr-native-fallback", "decision", captureSessionId, { error: String(cause) }, "degraded");
        await stopNativeVoiceCapture().catch(() => undefined);
        release();
      }
    }
    const browserEchoCancellation = echoCancellation && !native?.macosMajor;
    if (!started && echoCancellation && !browserEchoCancellation) {
      auditCapture("conversation-asr-browser-fallback", "decision", captureSessionId, { echoCancellation: false }, "degraded");
    }
    started ??= await startBrowserVoiceCapture(
      addFrame,
      (reason) => void stopConversationAsr(reason),
      inputDeviceId,
      browserEchoCancellation,
    );
    if (conversationAsrSnapshot().phase !== "starting") {
      await started.stop();
      return;
    }
    capture = started;
    publish({ phase: "recording" });
    updatePlaybackLimit();
    auditCapture("conversation-asr-capture-start", "terminal", captureSessionId, { backend, captureAecActive }, "success");
  } catch (cause) {
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
  auditCapture("conversation-asr-capture-stop", "request", captureSessionId, { reason: reason ?? "user" });
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
  await drainCompletion;
  auditCapture("conversation-asr-session-release", "request", captureSessionId);
  await releaseConversationAsrSession()
    .then(() => auditCapture("conversation-asr-session-release", "terminal", captureSessionId, {}, "success"))
    .catch((cause) => {
      const releaseError = String(cause);
      publish({ error: releaseError });
      auditCapture("conversation-asr-session-release", "error", captureSessionId, { error: releaseError }, "failure");
    });
  publish({ phase: "idle", error: error ?? state.error });
  auditCapture("conversation-asr-capture-stop", error ? "error" : "terminal", captureSessionId, error ? { error } : {}, error ? "failure" : "success");
  captureSessionId = null;
}
