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

export let state: CaptureState = {
  phase: "idle",
  error: null,
  entries: [],
  interimText: "",
  speechDetected: false,
  playbackLimited: false,
};
const listeners = new Set<() => void>();

export function subscribeConversationAsr(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function conversationAsrSnapshot() {
  return state;
}

export function publish(change: Partial<CaptureState>) {
  state = { ...state, ...change };
  listeners.forEach((listener) => listener());
}

export function updateEntry(id: string, change: Partial<CaptureEntry>) {
  publish({
    entries: state.entries.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)),
  });
}

export function failPendingQwenEntries() {
  if (
    !state.entries.some(
      (entry) => entry.status === "queued" && entry.provider === "Qwen ASR Realtime",
    )
  )
    return;
  publish({
    entries: state.entries.map((entry) =>
      entry.status === "queued" && entry.provider === "Qwen ASR Realtime"
        ? { ...entry, status: "failed", error: "Qwen ASRが確定文を返さずに終了しました。" }
        : entry,
    ),
  });
}
