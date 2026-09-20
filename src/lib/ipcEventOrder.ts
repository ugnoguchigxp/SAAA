import type { RuntimeEvent } from "./contracts";
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
