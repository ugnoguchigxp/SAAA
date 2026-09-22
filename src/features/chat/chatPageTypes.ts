import type { FormEvent } from "react";
import type {
  Conversation,
  ConversationMessage,
  ConversationVoicePolicySnapshot,
} from "../../lib/contracts";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";
import type { VoiceCaptureState } from "../voice/useAmbientVoiceSession";
import type { StreamingTextProjection } from "./streamingTextBuffer";
import type { RoutingEventRecord, RoutingSnapshot } from "../../lib/generated/runtimeEvent";
import type {
  RequiredContextFailureCode,
  RequiredContextRecoveryAction,
} from "./requiredContextRecovery";

export type ChatPageProps = {
  setupSnapshot?: import("../../lib/contracts").AppSnapshot;
  messages: ConversationMessage[];
  hasMoreMessages: boolean;
  loadingOlderMessages: boolean;
  hasNewerMessages?: boolean;
  loadingNewerMessages?: boolean;
  onLoadNewerMessages?: () => Promise<void>;
  onReturnToLatest: () => Promise<void>;
  onLoadOlderMessages: () => Promise<void>;
  streamingText: StreamingTextProjection;
  voiceState: VoiceCaptureState;
  voiceActivityLevel: number;
  voiceActivityDetected: boolean;
  listeningEnabled: boolean;
  runtimeActivity: ConversationRuntimeActivity[];
  composer: string;
  onComposerChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onToggleVoice: () => void;
  activeRunId: string | null;
  modelProviderStatus: {
    ready: boolean;
    label: string;
    location: "local" | "cloud" | null;
    state: "unchecked" | "active" | "ready" | "failed";
    fallbackUsed: boolean;
  };
  onOpenSettings: () => void;
  onStopRun: () => void;
  onStopSpeech: () => void;
  onRetry: () => void;
  selectedConversation: Conversation | undefined;
  activeTtsRunId: string | null;
  error: string | null;
  lastPrompt: string | null;
  retryKind: "response" | "speech" | null;
  requiredContextFailure: RequiredContextFailureCode | null;
  onPrepareRequiredContextRecovery: (action: RequiredContextRecoveryAction) => void;
  voicePolicy: ConversationVoicePolicySnapshot | null;
  voicePolicyUpdating: boolean;
  onSetConversationSpeechOutput: (value: "inherit" | "muted") => void;
  onSetConversationListeningPace: (value: "inherit" | "quick" | "balanced" | "patient") => void;
  onResetConversationVoiceOverrides: () => void;
  routingSnapshot: RoutingSnapshot;
  routingEvents: RoutingEventRecord[];
  routingCancellingRootId: string | null;
  routingDecidingProposalId: string | null;
  routingProposalError: string | null;
  onCancelRouting: (rootId: string) => void;
  onDecideRoutingProposal: (proposalId: string, candidateId: string, approve: boolean) => void;
};
