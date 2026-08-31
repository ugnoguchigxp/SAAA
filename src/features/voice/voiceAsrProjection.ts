import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";

export type VoiceAsrProjection = {
  sessionId: string | null;
  utteranceId: string | null;
  revision: number;
  stableText: string;
  unstableText: string;
  finalText: string | null;
  protocol: "native" | "batch-agreement" | null;
  scope: "all-speakers" | "target-speaker" | null;
  status: "idle" | "active" | "degraded" | "failed";
  reasonCode: string | null;
};

export const initialVoiceAsrProjection: VoiceAsrProjection = {
  sessionId: null,
  utteranceId: null,
  revision: 0,
  stableText: "",
  unstableText: "",
  finalText: null,
  protocol: null,
  scope: null,
  status: "idle",
  reasonCode: null,
};

export function projectVoiceAsrEvent(
  state: VoiceAsrProjection,
  event: VoiceAsrStreamEvent,
): VoiceAsrProjection {
  if (event.type === "ready") {
    return {
      ...initialVoiceAsrProjection,
      sessionId: event.sessionId,
      utteranceId: event.currentUtteranceId,
      protocol: event.protocol,
      scope: event.scope,
      status: "active",
    };
  }
  if (event.type === "stopped") {
    return event.sessionId === state.sessionId ? initialVoiceAsrProjection : state;
  }
  if (event.sessionId !== state.sessionId) return state;
  if (event.type === "degraded") {
    return {
      ...state,
      protocol: event.to,
      status: "degraded",
      reasonCode: event.reasonCode,
    };
  }
  if (event.type === "failed") {
    return {
      ...state,
      status: event.fatal ? "failed" : state.status,
      reasonCode: event.code,
    };
  }
  if (event.type === "partial") {
    if (state.utteranceId === event.utteranceId && event.revision <= state.revision) return state;
    return {
      ...state,
      utteranceId: event.utteranceId,
      revision: event.revision,
      stableText: event.stableText,
      unstableText: event.unstableText,
      finalText: null,
    };
  }
  if (event.type === "final") {
    if (state.utteranceId !== event.utteranceId) return state;
    if (state.utteranceId === event.utteranceId && event.revision <= state.revision) return state;
    return {
      ...state,
      utteranceId: event.utteranceId,
      revision: event.revision,
      stableText: "",
      unstableText: "",
      finalText: event.text,
    };
  }
  if (event.type === "utteranceDiscarded" && state.utteranceId === event.utteranceId) {
    return { ...state, stableText: "", unstableText: "", finalText: null };
  }
  return state;
}
