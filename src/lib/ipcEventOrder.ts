import type { MeetingEvent, RuntimeEvent } from "./contracts";
import type { VoiceAsrStreamEvent } from "./generated/voiceAsr";

export function runtimeEventOrder(runId: string) {
  let terminal = false;
  return (event: RuntimeEvent) => {
    if (event.runId !== runId) return false;
    // TTS runs independently and may finish after the conversation terminal.
    if (["speechStarted", "speechEnded", "speechFailed"].includes(event.type)) return true;
    if (terminal) return false;
    // Started may repeat when a provider attempt falls back to another provider.
    if (["messageCompleted", "cancelled", "failed"].includes(event.type)) terminal = true;
    return true;
  };
}

export function voiceAsrEventOrder(sessionId: string) {
  let ready = false;
  let stopped = false;
  let failed = false;
  const utterances = new Map<string, { revision: number; terminal: boolean }>();
  return (event: VoiceAsrStreamEvent) => {
    if (event.sessionId !== sessionId || stopped) return false;
    if (event.type === "ready") {
      if (ready) return false;
      ready = true;
      return true;
    }
    if (event.type === "stopped") {
      stopped = true;
      return true;
    }
    if (!ready) return false;
    // A fatal failure can still be followed by discard notifications and stopped.
    if (failed && event.type !== "utteranceDiscarded") return false;
    if (event.type === "failed") {
      failed = event.fatal;
      return true;
    }
    const previous = utterances.get(event.utteranceId);
    if (previous?.terminal) return false;
    if (event.type === "utteranceDiscarded") {
      utterances.set(event.utteranceId, { revision: previous?.revision ?? 0, terminal: true });
      return true;
    }
    if (event.revision <= (previous?.revision ?? 0)) return false;
    // Pending utterances may finish out of order, or without any partial event.
    utterances.set(event.utteranceId, {
      revision: event.revision,
      terminal: event.type === "final",
    });
    return true;
  };
}

export function meetingEventOrder() {
  let sessionId: string | null = null;
  let initialized = false;
  let terminal = false;
  const sequences = new Set<string>();
  return (event: MeetingEvent) => {
    if (event.type === "stateChanged") {
      if (event.sessionId !== sessionId) {
        sessionId = event.sessionId;
        terminal = false;
        sequences.clear();
      }
      const ended = sessionId !== null && ["completed", "failed", "idle"].includes(event.state);
      if (initialized && terminal && !ended) return false;
      initialized = true;
      terminal = ended;
      return true;
    }
    if (!initialized || event.sessionId !== sessionId) return false;
    if (event.type === "failed") {
      // The initial snapshot may already report failed before its notice arrives.
      terminal = true;
      return true;
    }
    if (terminal || !sessionId) return false;
    // Two lanes and multiple in-flight segments can complete out of order.
    const key = `${event.lane}:${event.sequence}`;
    if (sequences.has(key)) return false;
    sequences.add(key);
    return true;
  };
}
