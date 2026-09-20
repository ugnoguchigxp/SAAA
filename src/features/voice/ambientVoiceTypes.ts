import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { VoiceSettings, ConversationVoicePolicySnapshot } from "../../lib/contracts";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";
import type {
  ConversationSession,
  PendingConversationPrompt,
  SubmitPromptOptions,
} from "../../lib/conversationSession";
export type AmbientVoiceSessionOptions = {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  voicePolicy: ConversationVoicePolicySnapshot | null;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  setError: Dispatch<SetStateAction<string | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<ConversationRuntimeActivity[]>>;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
  persistListeningEnabled: (enabled: boolean) => Promise<void>;
};
