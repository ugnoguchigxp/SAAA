import { invoke } from "@tauri-apps/api/core";

import type { ConversationVoicePolicySnapshot } from "./contracts";

export const getConversationVoicePolicy = (
  conversationId: string,
): Promise<ConversationVoicePolicySnapshot> =>
  invoke("get_conversation_voice_policy", { conversationId });

export const updateConversationVoicePolicy = (input: {
  conversationId: string;
  speechOutput: "inherit" | "muted" | null;
  listeningPace: "inherit" | "quick" | "balanced" | "patient" | null;
  expectedRevision: number;
}): Promise<ConversationVoicePolicySnapshot> =>
  invoke("update_conversation_voice_policy", { input });

export const resetConversationVoicePolicy = (input: {
  conversationId: string;
  expectedRevision: number;
}): Promise<ConversationVoicePolicySnapshot> =>
  invoke("reset_conversation_voice_policy", { input });
