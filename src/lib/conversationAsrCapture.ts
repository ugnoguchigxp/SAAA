import { stageAudioUpload } from "./audioIpc";
import { startBrowserVoiceCapture, type BrowserVoiceCapture } from "./browserVoiceCapture";
import { microphoneErrorMessage } from "./microphone";
import { releaseConversationAsrSession, transcribeConversationAudio } from "./runtime";

const CHUNK_SAMPLES = 16_000 * 10;
const MAX_PENDING_CHUNKS = 24;

export type CaptureEntry = {
  id: string;
  recordedAt: string;
  seconds: number;
  status: "queued" | "transcribing" | "completed" | "failed";
  text: string | null;
  language: string | null;
  provider: string | null;
  error: string | null;
};

type CaptureState = {
  phase: "idle" | "starting" | "recording" | "stopping";
  error: string | null;
  entries: CaptureEntry[];
};

let state: CaptureState = { phase: "idle", error: null, entries: [] };
const listeners = new Set<() => void>();
let frames: Float32Array[] = [];
let sampleCount = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
let capture: BrowserVoiceCapture | null = null;
// Serialize uploads so a slow ASR response never exhausts the bounded staging store.
let transcriptionQueue: Promise<void> = Promise.resolve();
let pendingChunks = 0;

export function subscribeConversationAsr(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
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
    entries: state.entries.map((entry) =>
      entry.id === id ? { ...entry, ...change } : entry,
    ),
  });
}

function addFrame(frame: Float32Array) {
  let offset = 0;
  while (offset < frame.length) {
    const length = Math.min(frame.length - offset, CHUNK_SAMPLES - sampleCount);
    frames.push(frame.slice(offset, offset + length));
    sampleCount += length;
    offset += length;
    if (sampleCount === CHUNK_SAMPLES) flushChunk();
  }
}

function scheduleChunk() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    flushChunk();
    if (state.phase === "recording" && timer === null) scheduleChunk();
  }, 10_000);
}

function flushChunk(reason?: string) {
  if (sampleCount === 0) return;
  const samples = new Float32Array(sampleCount);
  let offset = 0;
  for (const frame of frames) {
    samples.set(frame, offset);
    offset += frame.length;
  }
  frames = [];
  sampleCount = 0;
  if (state.phase === "recording") scheduleChunk();
  const id = crypto.randomUUID();
  const entry: CaptureEntry = {
    id,
    recordedAt: new Date().toLocaleString(),
    seconds: samples.length / 16_000,
    status: "queued",
    text: null,
    language: null,
    provider: null,
    error: reason ?? null,
  };
  publish({ entries: [entry, ...state.entries] });
  if (samples.length < 1_600) {
    updateEntry(id, { status: "failed", error: reason ?? "録音が0.1秒未満です。" });
    samples.fill(0);
    return;
  }
  if (pendingChunks >= MAX_PENDING_CHUNKS) {
    updateEntry(id, {
      status: "failed",
      error: "ASR処理が滞留しているため、この区間を送信できませんでした。",
    });
    samples.fill(0);
    return;
  }
  pendingChunks += 1;
  transcriptionQueue = transcriptionQueue.then(async () => {
    try {
      updateEntry(id, { status: "transcribing" });
      const uploadId = await stageAudioUpload(samples, "conversation-asr");
      samples.fill(0);
      const result = await transcribeConversationAudio(uploadId);
      updateEntry(id, {
        status: "completed",
        text: result.text,
        language: result.language,
        provider: result.providerLabel,
      });
    } catch (cause) {
      updateEntry(id, {
        status: "failed",
        error: [reason, String(cause)].filter(Boolean).join(" · "),
      });
    } finally {
      samples.fill(0);
      pendingChunks -= 1;
    }
  });
}

export async function startConversationAsr(inputDeviceId: string, echoCancellation: boolean) {
  if (state.phase !== "idle") return;
  publish({ phase: "starting", error: null });
  try {
    const started = await startBrowserVoiceCapture(
      addFrame,
      (reason) => void stopConversationAsr(reason),
      inputDeviceId,
      echoCancellation,
    );
    if (conversationAsrSnapshot().phase !== "starting") {
      await started.stop();
      return;
    }
    capture = started;
    publish({ phase: "recording" });
    scheduleChunk();
  } catch (cause) {
    if (timer) clearTimeout(timer);
    timer = null;
    const message = microphoneErrorMessage(cause);
    flushChunk(message);
    publish({ phase: "idle", error: message });
  }
}

export async function stopConversationAsr(reason?: string) {
  if (state.phase !== "recording" && state.phase !== "starting") return;
  publish({ phase: "stopping" });
  if (timer) clearTimeout(timer);
  timer = null;
  let error = reason ?? null;
  const current = capture;
  capture = null;
  try {
    await current?.stop();
  } catch (cause) {
    error = [error, String(cause)].filter(Boolean).join(" · ");
  }
  flushChunk(error ?? undefined);
  publish({ phase: "idle", error });
  transcriptionQueue = transcriptionQueue.then(releaseConversationAsrSession).catch((cause) => {
    publish({ error: String(cause) });
  });
}
