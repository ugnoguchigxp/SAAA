import type { FormEvent } from "react";
import type { Conversation, ConversationMessage, ConversationVoicePolicySnapshot } from "../../lib/contracts";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";
import type { VoiceCaptureState } from "../voice/useAmbientVoiceSession";
import type { VoiceAsrProjection } from "../voice/voiceAsrProjection";
import type { StreamingTextProjection } from "./streamingTextBuffer";

export type ChatPageProps = {
  messages: ConversationMessage[]; hasMoreMessages: boolean; loadingOlderMessages: boolean;
  onLoadOlderMessages: () => Promise<void>; streamingText: StreamingTextProjection;
  interimTranscript: { text: string; projection: VoiceAsrProjection }; voiceState: VoiceCaptureState;
  listeningEnabled: boolean; runtimeActivity: ConversationRuntimeActivity[]; composer: string;
  onComposerChange: (value: string) => void; onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onToggleVoice: () => void; voiceStarting: boolean; meetingActive: boolean; activeRunId: string | null;
  modelProviderStatus: { ready: boolean; label: string; location: "local" | "cloud" | null; state: "unchecked" | "active" | "ready" | "failed"; fallbackUsed: boolean };
  onOpenSettings: () => void; onOpenMeeting: () => void; onOpenSituation: () => void;
  onStopRun: () => void; onStopSpeech: () => void; onRetry: () => void;
  selectedConversation: Conversation | undefined; activeTtsRunId: string | null; error: string | null;
  lastPrompt: string | null; retryKind: "response" | "speech" | null;
  voicePolicy: ConversationVoicePolicySnapshot | null; voicePolicyUpdating: boolean;
  onSetConversationSpeechOutput: (value: "inherit" | "muted") => void;
  onSetConversationListeningPace: (value: "inherit" | "quick" | "balanced" | "patient") => void;
  onResetConversationVoiceOverrides: () => void;
};
