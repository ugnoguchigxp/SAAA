import "./tauriCoreMock";
import type { MutableRefObject } from "react";
import type { VoiceSettings } from "../src/lib/contracts";
import type {
  ConversationSession,
  PendingConversationPrompt,
} from "../src/lib/conversationSession";
import { voiceSettings, voicePolicy } from "./ambientVoiceFixtures";
await import("../src/i18n");
const { useAmbientVoiceSession } = await import("../src/features/voice/useAmbientVoiceSession");

export type SessionApi = ReturnType<typeof useAmbientVoiceSession>;

export function Harness({
  apiRef,
  conversationId = "conversation-1",
  meetingState = "idle",
  settings = { ...voiceSettings, listeningEnabled: true },
  sessionRef,
  pendingRef,
}: {
  apiRef: MutableRefObject<SessionApi | null>;
  conversationId?: string | null;
  meetingState?: "idle" | "active";
  settings?: VoiceSettings | null;
  sessionRef: MutableRefObject<ConversationSession>;
  pendingRef: MutableRefObject<PendingConversationPrompt[]>;
}) {
  apiRef.current = useAmbientVoiceSession({
    selectedConversationId: conversationId,
    voiceSettings: settings,
    voicePolicy,
    meetingState,
    conversationSessionRef: sessionRef,
    pendingVoicePromptsRef: pendingRef,
    setError: () => undefined,
    setRuntimeActivity: (update) => (typeof update === "function" ? update([]) : update),
    stopSpeech: async () => undefined,
    submitPrompt: async (prompt) => {
      submitted.push(prompt);
    },
    persistListeningEnabled: async () => undefined,
  });
  return null;
}

export const submitted: string[] = [];
