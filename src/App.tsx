import { useCommittedCallback } from "./useCommittedCallback";
import {
  Suspense,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { useTranslation } from "react-i18next";
import "./App.css";
import { ChatPage } from "./features/chat/ChatPage";
import { useConversationTurn } from "./features/chat/useConversationTurn";
import { useAmbientVoiceSession } from "./features/voice/useAmbientVoiceSession";
import { toMessage } from "./lib/appHelpers";
import { findSettingsDocument, type AppSnapshot } from "./lib/contracts";
import { voiceSettingsSchema } from "./lib/schemas";
import { applySnapshotLanguage } from "./lib/appLanguage";
import { uiMessage } from "./i18n/presentation";
import { resolveModelProviderStatus } from "./lib/conversationRouting";
import type { ConversationRuntimeActivity } from "./lib/conversationActivity";
import {
  initialConversationSession,
  type ConversationSession,
  type PendingConversationPrompt,
  type SubmitPromptOptions,
} from "./lib/conversationSession";
import { getAppSnapshot, reportFrontendReady, setVoiceListeningEnabled } from "./lib/runtime";
import { useWindowShortcut } from "./useWindowShortcut";
import { useAppErrors } from "./useAppErrors";
import { useOwnedSignalHeartbeat } from "./useOwnedSignalHeartbeat";
import { SettingsPage } from "./appPages";
import { DesignSystemProvider } from "./design-system";
import "./design-system/styles.css";

type Surface = "chat" | "settings";
const initialSnapshot: AppSnapshot = {
  settings: [],
  conversations: [],
  primaryConversationId: "",
  effectiveRoute: {
    providerId: null,
    label: "Model not selected",
    location: null,
    state: "unchecked",
    fallbackUsed: false,
    reasonCode: "snapshot-loading",
    updatedAt: null,
  },
  voiceProfile: {
    status: "empty",
    filterEnabled: false,
    runtimeAvailable: false,
    runtimeMessage: "Loading local speaker verification…",
    sampleCount: 0,
    targetSampleCount: 5,
    totalDurationMs: 0,
    minimumDurationMs: 20_000,
    threshold: 0.55,
    samples: [],
  },
};

function App() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<AppSnapshot>(initialSnapshot);
  const [surface, setSurface] = useState<Surface>("chat");
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const { errors, error, setAppError, setConversationError, setVoiceError } = useAppErrors();
  const pendingVoicePromptsRef = useRef<PendingConversationPrompt[]>([]);
  const conversationSessionRef = useRef<ConversationSession>(initialConversationSession);
  const submitPromptRef = useRef<(prompt: string, options?: SubmitPromptOptions) => Promise<void>>(
    async () => {},
  );
  const stopSpeechRef = useRef<() => Promise<void>>(async () => {});
  const suspendVoiceForSpeechRef = useRef<(runId: string) => Promise<boolean>>(async () => false);
  const resumeVoiceAfterSpeechRef = useRef<(runId: string) => Promise<void>>(async () => {});
  const setRuntimeActivityRef = useRef<Dispatch<SetStateAction<ConversationRuntimeActivity[]>>>(
    () => {},
  );
  const selectedConversation = snapshot.conversations.find(
    (conversation) => conversation.id === selectedConversationId,
  );
  const modelProviderStatus = useMemo(() => resolveModelProviderStatus(snapshot), [snapshot]);
  const voiceSettings = useMemo(() => {
    const document = findSettingsDocument(snapshot.settings, "voice.runtime", "default");
    const parsed = voiceSettingsSchema.safeParse(document?.valueJson);
    return parsed.success ? parsed.data : null;
  }, [snapshot.settings]);
  const turn = useConversationTurn({
    selectedConversationId,
    voiceSettings,
    pendingVoicePromptsRef,
    conversationSessionRef,
    suspendVoiceForSpeech: (runId) => suspendVoiceForSpeechRef.current(runId),
    resumeVoiceAfterSpeech: (runId) => resumeVoiceAfterSpeechRef.current(runId),
    setSnapshot,
    setError: setConversationError,
  });
  const voice = useAmbientVoiceSession({
    selectedConversationId,
    voiceSettings,
    voicePolicy: turn.voicePolicy,
    conversationSessionRef,
    pendingVoicePromptsRef,
    setError: setVoiceError,
    setRuntimeActivity: (value) => setRuntimeActivityRef.current(value),
    stopSpeech: () => stopSpeechRef.current(),
    submitPrompt: (prompt, options) => submitPromptRef.current(prompt, options),
    persistListeningEnabled: async (enabled) => {
      const document = await setVoiceListeningEnabled(enabled);
      setSnapshot((current) => ({
        ...current,
        settings: current.settings.some(
          (item) => item.namespace === document.namespace && item.key === document.key,
        )
          ? current.settings.map((item) =>
              item.namespace === document.namespace && item.key === document.key ? document : item,
            )
          : [...current.settings, document],
      }));
    },
  });
  suspendVoiceForSpeechRef.current = voice.suspendVoiceForSpeech;
  resumeVoiceAfterSpeechRef.current = voice.resumeVoiceAfterSpeech;
  submitPromptRef.current = turn.submitPrompt;
  stopSpeechRef.current = turn.stopSpeech;
  setRuntimeActivityRef.current = turn.setRuntimeActivity;
  const { voiceBusy, voiceState } = voice;
  const { activeRunId, activeTtsRunId, composer } = turn;
  const stopSpeech = turn.stopSpeech;

  const initializeCommitted = useCommittedCallback(initialize);
  useEffect(() => {
    void reportFrontendReady();
    void initializeCommitted();
  }, [initializeCommitted]);
  useWindowShortcut((event) => {
    const command = event.metaKey || event.ctrlKey;
    if (command && event.key === ",") {
      event.preventDefault();
      if (surface === "settings") openChatSurface();
      else openSettings();
    } else if (event.key === "Escape") {
      if (surface === "settings") {
        event.preventDefault();
        openChatSurface();
        return;
      }
      if (activeRunId) void turn.stopActiveRun();
      if (voice.listeningEnabled || voiceState !== "idle") void voice.toggleAmbientListening();
      if (activeTtsRunId) void stopSpeech();
    }
  });
  useOwnedSignalHeartbeat({ activeRunId, activeTtsRunId, composer, voiceState });

  async function initialize() {
    try {
      setLoading(true);
      setAppError(null);
      const nextSnapshot = await getAppSnapshot();
      const primaryConversation = nextSnapshot.conversations.find(
        (conversation) => conversation.id === nextSnapshot.primaryConversationId,
      );
      if (!primaryConversation) throw new Error(uiMessage("appPrimaryConversationUnavailable"));
      applySnapshotLanguage(nextSnapshot);
      setSnapshot(nextSnapshot);
      setSelectedConversationId(primaryConversation.id);
    } catch (cause) {
      setAppError(toMessage(cause));
    } finally {
      setLoading(false);
    }
  }

  async function refreshSnapshot() {
    try {
      const next = await getAppSnapshot();
      applySnapshotLanguage(next);
      setSnapshot(next);
      setAppError(null);
    } catch (cause) {
      setAppError(toMessage(cause));
    }
  }

  function canChangeConversation(): boolean {
    if (conversationSessionRef.current.runId) {
      setAppError(uiMessage("appSurfaceSwitchBlocked"));
      return false;
    }
    setAppError(null);
    return true;
  }

  function openChatSurface() {
    if (conversationSessionRef.current.runId) {
      setAppError(null);
      setSurface("chat");
      return;
    }
    setAppError(null);
    setSurface("chat");
  }

  function openSettings() {
    if (!canChangeConversation()) return;
    setSurface("settings");
  }

  if (loading) return <main className="boot-screen">{t("app.booting")}</main>;
  return (
    <main className="app-shell">
      <Suspense fallback={<main className="boot-screen">{t("app.booting")}</main>}>
        {surface === "settings" ? (
          <SettingsPage
            documents={snapshot.settings}
            voiceProfile={snapshot.voiceProfile}
            voiceEnrollmentBlocked={voiceBusy || Boolean(activeTtsRunId)}
            voiceListeningEnabled={voice.listeningEnabled}
            voiceListeningBusy={voice.voiceActionInProgress}
            voiceAvailability={voice.voiceAvailability}
            voiceError={errors.voice}
            onClose={openChatSurface}
            onToggleVoiceListening={(enabled) => void voice.toggleAmbientListening(enabled)}
            onSaved={(settings) => {
              setSnapshot((current) => ({ ...current, settings }));
              void refreshSnapshot();
            }}
            onVoiceProfileChanged={(voiceProfile) =>
              setSnapshot((current) => ({ ...current, voiceProfile }))
            }
          />
        ) : (
          <ChatPage
            setupSnapshot={snapshot}
            messages={turn.messages}
            hasMoreMessages={turn.hasMoreMessages}
            loadingOlderMessages={turn.loadingOlderMessages}
            onLoadOlderMessages={turn.loadOlderMessages}
            hasNewerMessages={turn.hasNewerMessages}
            loadingNewerMessages={turn.loadingNewerMessages}
            onLoadNewerMessages={turn.loadNewerMessages}
            streamingText={turn.streamingText}
            interimTranscript={voice.interimTranscript}
            voiceState={voiceState}
            listeningEnabled={voice.listeningEnabled}
            runtimeActivity={turn.runtimeActivity}
            composer={composer}
            onComposerChange={turn.setComposer}
            onSubmit={(event) => void turn.handleSubmit(event)}
            onToggleVoice={() => void voice.toggleAmbientListening()}
            voiceStarting={voice.voiceStarting}
            activeRunId={activeRunId}
            modelProviderStatus={modelProviderStatus}
            onOpenSettings={openSettings}
            onStopRun={() => void turn.stopActiveRun()}
            onStopSpeech={() => void stopSpeech()}
            onRetry={() => void turn.retryFailedAction()}
            selectedConversation={selectedConversation}
            activeTtsRunId={activeTtsRunId}
            error={error}
            lastPrompt={turn.lastPrompt}
            retryKind={turn.retryKind}
            voicePolicy={turn.voicePolicy}
            voicePolicyUpdating={turn.voicePolicyUpdating}
            onSetConversationSpeechOutput={(value) => void turn.setConversationSpeechOutput(value)}
            onSetConversationListeningPace={(value) =>
              void turn.setConversationListeningPace(value)
            }
            onResetConversationVoiceOverrides={() => void turn.resetConversationVoiceOverrides()}
          />
        )}
      </Suspense>
    </main>
  );
}

export default function DesignSystemApp() {
  return (
    <DesignSystemProvider>
      <App />
    </DesignSystemProvider>
  );
}
