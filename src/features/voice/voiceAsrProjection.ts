import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";
export type VoiceAsrProjection = { sessionId: string | null; utteranceId: string | null; revision: number; stableText: string; unstableText: string; finalText: string | null };
export const initialVoiceAsrProjection: VoiceAsrProjection = { sessionId: null, utteranceId: null, revision: 0, stableText: "", unstableText: "", finalText: null };
export function projectVoiceAsrEvent(state: VoiceAsrProjection, event: VoiceAsrStreamEvent): VoiceAsrProjection {
  if (event.type === "ready") return { ...initialVoiceAsrProjection, sessionId: event.sessionId, utteranceId: event.currentUtteranceId };
  if (event.type === "stopped") return event.sessionId === state.sessionId ? initialVoiceAsrProjection : state;
  if (!("sessionId" in event) || event.sessionId !== state.sessionId) return state;
  if (event.type === "partial") {
    if (state.utteranceId === event.utteranceId && event.revision <= state.revision) return state;
    return { ...state, utteranceId: event.utteranceId, revision: event.revision, stableText: event.stableText, unstableText: event.unstableText, finalText: null };
  }
  if (event.type === "final") {
    if (state.utteranceId === event.utteranceId && event.revision < state.revision) return state;
    return { ...state, utteranceId: event.utteranceId, revision: event.revision, stableText: "", unstableText: "", finalText: event.text };
  }
  if (event.type === "utteranceDiscarded" && state.utteranceId === event.utteranceId) return { ...state, stableText: "", unstableText: "", finalText: null };
  return state;
}
