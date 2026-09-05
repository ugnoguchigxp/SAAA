import { useEffect } from "react";
import { reportOwnedSignal } from "./lib/runtime";

export function useOwnedSignalHeartbeat({ activeRunId, activeTtsRunId, composer, meetingState, voiceState }: {
  activeRunId: string | null;
  activeTtsRunId: string | null;
  composer: string;
  meetingState: string;
  voiceState: string;
}) {
  useEffect(() => {
    const input = {
      conversationState: activeRunId ? "model-running" : composer.trim() ? "user-input" : "idle",
      microphoneState: meetingState === "active" || voiceState === "recording" ? "saaa-capturing" : voiceState === "transcribing" ? "saaa-transcribing" : "inactive",
      audioState: activeTtsRunId ? "saaa-speaking" : "silent",
    } as const;
    void reportOwnedSignal(input).catch(() => undefined);
    if (input.conversationState === "idle" && input.microphoneState === "inactive" && input.audioState === "silent") return;
    const heartbeat = window.setInterval(() => { void reportOwnedSignal(input).catch(() => undefined); }, 2_000);
    return () => window.clearInterval(heartbeat);
  }, [activeRunId, activeTtsRunId, composer, meetingState, voiceState]);
}
