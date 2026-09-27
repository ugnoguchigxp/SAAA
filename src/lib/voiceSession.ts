export type FinalizeMode = "continue" | "stop";

export type VoiceSession = {
  actionInProgress: boolean;
  capture: "idle" | "starting" | "recording" | "suspended";
  finalizing: boolean;
  pendingFinalize: FinalizeMode | null;
};

export type VoiceSessionEvent =
  | { type: "actionStarted" | "actionFinished" }
  | { type: "captureStarting" | "captureStarted" | "captureDetached" | "captureSuspended" }
  | { type: "finalizeRequested"; mode: FinalizeMode }
  | { type: "finalizeCompleted" };

export const initialVoiceSession: VoiceSession = {
  actionInProgress: false,
  capture: "idle",
  finalizing: false,
  pendingFinalize: null,
};

export function transitionVoiceSession(
  state: VoiceSession,
  event: VoiceSessionEvent,
): VoiceSession {
  switch (event.type) {
    case "actionStarted":
      return { ...state, actionInProgress: true };
    case "actionFinished":
      return { ...state, actionInProgress: false };
    case "captureStarting":
      return { ...state, capture: "starting" };
    case "captureStarted":
      return { ...state, capture: "recording" };
    case "captureDetached":
      return { ...state, capture: "idle" };
    case "captureSuspended":
      return {
        ...state,
        capture: ["starting", "recording"].includes(state.capture) ? "suspended" : state.capture,
      };
    case "finalizeRequested":
      return state.finalizing
        ? { ...state, pendingFinalize: mergeFinalizeMode(state.pendingFinalize, event.mode) }
        : { ...state, finalizing: true };
    case "finalizeCompleted":
      return { ...state, finalizing: false, pendingFinalize: null };
  }
}

export type VoiceCaptureState = "stopped" | "preparing" | "listening";

export function voiceCaptureState(
  state: VoiceSession,
  listeningEnabled: boolean,
): VoiceCaptureState {
  if (!listeningEnabled) {
    return state.actionInProgress || state.capture !== "idle" || state.finalizing
      ? "preparing"
      : "stopped";
  }
  if (state.capture === "recording" && !state.finalizing) return "listening";
  // A paused microphone is not a new ASR start. "preparing" is only for a start still in progress.
  if (state.capture === "suspended") return "listening";
  return "preparing";
}

export function voiceSessionBusy(state: VoiceSession): boolean {
  return voiceSessionProcessing(state) || state.capture !== "idle";
}

export function voiceSessionProcessing(state: VoiceSession): boolean {
  return state.actionInProgress || state.finalizing;
}

function mergeFinalizeMode(current: FinalizeMode | null, next: FinalizeMode): FinalizeMode {
  return current === "stop" || next === "stop" ? "stop" : "continue";
}
