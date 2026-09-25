import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../../lib/contracts";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { withTimeout } from "../../lib/promiseTimeout";
import type { CommitReason } from "../../lib/generated/voiceAsr";
import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";
import type { VoiceSession, VoiceSessionEvent } from "../../lib/voiceSession";
import type { SubmitPromptOptions } from "../../lib/conversationSession";
import type { ConversationSession } from "../../lib/conversationSession";
import {
  appendVoiceAsrAudio,
  commitVoiceAsrUtterance,
  startVoiceAsrSession,
  stopVoiceAsrSession,
} from "../../lib/voiceAsrRuntime";
import { interruptNativeVoicePlayback } from "../../lib/audioBackend";
import { attachAmbientVoiceCapture, resetVoiceActivityDetector } from "./ambientVoiceCapture";
import { VoiceAsrPacketSender } from "./voiceAsrPacketSender";
import { projectVoiceAsrEvent } from "./voiceAsrProjection";
import { effectiveCaptureSettings, voiceStartupMessage } from "./voiceCaptureSettings";
import {
  auditCaptureCancelled,
  auditCaptureFailed,
  auditCaptureStarted,
  auditCaptureSuspended,
  auditVoiceDeliveryBlocked,
  auditVoiceDeliveryDecision,
  auditVoiceDeliverySettlement,
} from "./voiceAudit";
import type { VoiceCaptureResources } from "./VoiceCaptureResources";

const ASR_SAMPLE_RATE = 16_000;
export type SuspensionReason = "speech";

export function createAmbientVoiceCaptureActions(input: {
  resources: VoiceCaptureResources;
  applyVoiceEvent: (event: VoiceSessionEvent) => void;
  updateListeningEnabled: (enabled: boolean) => void;
  persistListeningEnabled: (enabled: boolean) => Promise<void>;
  setError: Dispatch<SetStateAction<string | null>>;
  setInterimTranscript: Dispatch<SetStateAction<string>>;
  setVoiceActivityDetected: Dispatch<SetStateAction<boolean>>;
  setVoiceActivityLevel: Dispatch<SetStateAction<number>>;
  setAsrProjection: Dispatch<SetStateAction<ReturnType<typeof projectVoiceAsrEvent>>>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
  selectedConversationIdRef: MutableRefObject<string | null>;
  voiceSettingsRef: MutableRefObject<VoiceSettings | null>;
  voicePolicyRef: MutableRefObject<ConversationVoicePolicySnapshot | null>;
  voiceSessionRef: MutableRefObject<VoiceSession>;
  suspensionReasonRef: MutableRefObject<SuspensionReason | null>;
  speechResumeTokenRef: MutableRefObject<string | null>;
  voiceActivityLevelRef: MutableRefObject<number>;
  voiceActivityDetectedRef: MutableRefObject<boolean>;
  voiceActivityUpdatedAtRef: MutableRefObject<number>;
  pendingVoiceDeliveriesRef: MutableRefObject<number>;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  ttsStartedAtRef: MutableRefObject<number>;
  stopSpeech: () => Promise<void>;
}) {
  const {
    voiceStreamRef,
    voiceContextRef,
    voiceSourceRef,
    voiceNodeRef,
    voiceFlushResolverRef,
    voiceActivityDetectorRef,
    voiceCaptureLeaseRef,
    voiceCaptureAttemptRef,
    voiceAsrPacketizerRef,
    voiceAsrSenderRef,
    voiceAsrSessionIdRef,
    acceptedVoiceAsrSessionsRef,
    voiceAsrConversationsRef,
    voiceAsrStopWaitersRef,
    voiceAsrPacketCountRef,
    voiceAsrProjectionRef,
    voiceFinalDeliveryRef,
    nativeCaptureRef,
    nativeStopRef,
    disposedRef,
    listeningEnabledRef,
    detachVoiceCapture,
    waitForVoiceAsrStopped,
    packetVoiceFrame,
  } = input.resources;
  const {
    applyVoiceEvent,
    updateListeningEnabled,
    persistListeningEnabled,
    setError,
    setInterimTranscript,
    setVoiceActivityDetected,
    setVoiceActivityLevel,
    setAsrProjection,
    submitPrompt,
    selectedConversationIdRef,
    voiceSettingsRef,
    voicePolicyRef,
    voiceSessionRef,
    suspensionReasonRef,
    speechResumeTokenRef,
    voiceActivityLevelRef,
    voiceActivityDetectedRef,
    voiceActivityUpdatedAtRef,
    pendingVoiceDeliveriesRef,
    conversationSessionRef,
    ttsStartedAtRef,
    stopSpeech,
  } = input;

  async function attachVoiceCapture() {
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (
      disposedRef.current ||
      !settings ||
      !selectedConversationIdRef.current ||
      voiceStreamRef.current ||
      nativeCaptureRef.current ||
      suspensionReasonRef.current
    )
      return;
    if (
      voiceSessionRef.current.capture === "starting" ||
      voiceSessionRef.current.capture === "recording"
    )
      return;
    const sessionId = crypto.randomUUID();
    voiceAsrSessionIdRef.current = sessionId;
    acceptedVoiceAsrSessionsRef.current.add(sessionId);
    voiceAsrConversationsRef.current.set(sessionId, selectedConversationIdRef.current);
    applyVoiceEvent({ type: "captureStarting" });
    try {
      const start = (recoverExisting: boolean) =>
        startVoiceAsrSession(
          {
            sessionId,
            conversationId: selectedConversationIdRef.current!,
            sampleRate: ASR_SAMPLE_RATE,
            recoverExisting,
          },
          handleVoiceAsrEvent,
        );
      try {
        await start(false);
      } catch (cause) {
        if (toMessage(cause) !== "asr-session-exists") throw cause;
        await start(true);
      }
      if (
        disposedRef.current ||
        voiceAsrSessionIdRef.current !== sessionId ||
        !acceptedVoiceAsrSessionsRef.current.has(sessionId)
      ) {
        auditCaptureCancelled(sessionId, voiceAsrConversationsRef.current.get(sessionId) ?? null);
        await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
        return;
      }
      voiceAsrPacketizerRef.current.reset();
      voiceAsrPacketCountRef.current = 0;
      voiceAsrSenderRef.current = new VoiceAsrPacketSender(
        {
          append: (sequence, bytes) => appendVoiceAsrAudio(sessionId, sequence, bytes),
          commit: (reason) => commitVoiceAsrUtterance({ sessionId, reason }),
          stop: (finalizeCurrent) => stopVoiceAsrSession({ sessionId, finalizeCurrent }),
        },
        (error) => {
          setError((current) => current ?? error.message);
          void terminateFailedVoiceCapture();
        },
      );
      await attachAmbientVoiceCapture({
        settings,
        disposed: disposedRef,
        listeningEnabled: listeningEnabledRef,
        captureAttempt: voiceCaptureAttemptRef,
        stream: voiceStreamRef,
        audioContext: voiceContextRef,
        source: voiceSourceRef,
        node: voiceNodeRef,
        flushResolver: voiceFlushResolverRef,
        activityDetector: voiceActivityDetectorRef,
        captureLease: voiceCaptureLeaseRef,
        applyEvent: applyVoiceEvent,
        finishSegment: (reason) => void finishVoiceCapture(true, reason),
        packetFrame: packetVoiceFrame,
        packetCount: () => voiceAsrPacketCountRef.current,
        clearTranscript: () => setInterimTranscript(""),
        onActivity: ({ hasSpeech, rms }) => {
          if (disposedRef.current) return;
          if (hasSpeech !== voiceActivityDetectedRef.current) {
            voiceActivityDetectedRef.current = hasSpeech;
            setVoiceActivityDetected(hasSpeech);
          }
          const level = Math.max(0, Math.min(1, (rms - 0.003) / 0.027));
          const now = performance.now();
          if (
            now - voiceActivityUpdatedAtRef.current < 60 &&
            Math.abs(level - voiceActivityLevelRef.current) < 0.12
          )
            return;
          voiceActivityUpdatedAtRef.current = now;
          voiceActivityLevelRef.current = level;
          setVoiceActivityLevel(level);
        },
        nativeCapture: nativeCaptureRef,
        nativeStop: nativeStopRef,
        bargeInEnabled: settings.bargeInEnabled,
        speechIsPlaying: () => Boolean(conversationSessionRef.current.speechRunId),
        ttsStartedAtMs: () => ttsStartedAtRef.current,
        interruptSpeech: () => {
          void interruptNativeVoicePlayback();
          void stopSpeech();
        },
      });
      auditCaptureStarted(
        sessionId,
        selectedConversationIdRef.current,
        voiceSessionRef.current.capture,
      );
      // applyEvent inside the capture attach moves the ref to "recording". The early
      // return above narrowed the type before that write, so read it again.
      const capture = voiceSessionRef.current.capture as VoiceSession["capture"];
      if (suspensionReasonRef.current && capture === "recording") {
        await suspendVoice(suspensionReasonRef.current);
      }
    } catch (cause) {
      auditCaptureFailed(sessionId, voiceAsrConversationsRef.current.get(sessionId) ?? null, cause);
      acceptedVoiceAsrSessionsRef.current.delete(sessionId);
      voiceAsrConversationsRef.current.delete(sessionId);
      applyVoiceEvent({ type: "captureDetached" });
      await detachVoiceCapture(false);
      if (listeningEnabledRef.current) {
        updateListeningEnabled(false);
        await persistListeningEnabled(false).catch(() => undefined);
      }
      if (!disposedRef.current) setError(voiceStartupMessage(cause));
    }
  }

  async function suspendVoice(reason: SuspensionReason): Promise<boolean> {
    if (voiceSessionRef.current.capture === "suspended") {
      suspensionReasonRef.current = reason;
      return true;
    }
    // Speech can arrive while the utterance commit or the ASR session start is still
    // in flight. Detaching there cancels the start. Hold the pause until that work finishes.
    if (voiceSessionRef.current.finalizing || voiceSessionRef.current.capture === "starting") {
      suspensionReasonRef.current = reason;
      return true;
    }
    if (!voiceStreamRef.current && !nativeCaptureRef.current) return false;
    suspensionReasonRef.current = reason;
    auditCaptureSuspended(voiceAsrSessionIdRef.current, selectedConversationIdRef.current, reason);
    applyVoiceEvent({ type: "captureSuspended" });
    await detachVoiceCapture(false);
    return true;
  }

  async function suspendVoiceForSpeech(speechRunId: string): Promise<boolean> {
    speechResumeTokenRef.current = speechRunId;
    ttsStartedAtRef.current = performance.now();
    if (nativeCaptureRef.current && voiceSettingsRef.current?.bargeInEnabled !== false) {
      return true;
    }
    return suspendVoice("speech");
  }

  async function resumeVoice(reason: SuspensionReason): Promise<void> {
    if (disposedRef.current || suspensionReasonRef.current !== reason) return;
    if (!listeningEnabledRef.current) {
      suspensionReasonRef.current = null;
      applyVoiceEvent({ type: "captureDetached" });
      return;
    }
    suspensionReasonRef.current = null;
    await attachVoiceCapture();
  }

  async function resumeVoiceAfterSpeech(speechRunId: string): Promise<void> {
    if (speechResumeTokenRef.current !== speechRunId) return;
    speechResumeTokenRef.current = null;
    if (suspensionReasonRef.current === "speech") {
      await resumeVoice("speech");
      return;
    }
    if (listeningEnabledRef.current && voiceSessionRef.current.capture === "idle")
      await attachVoiceCapture();
  }

  async function finishVoiceCapture(keepListening: boolean, reason: CommitReason = "silence") {
    if (disposedRef.current) return;
    const mode = keepListening ? "continue" : "stop";
    if (voiceSessionRef.current.finalizing) {
      applyVoiceEvent({ type: "finalizeRequested", mode });
      return;
    }
    applyVoiceEvent({ type: "finalizeRequested", mode });
    const conversationId = selectedConversationIdRef.current;
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (!conversationId || !settings) {
      try {
        await detachVoiceCapture(false);
      } finally {
        applyVoiceEvent({ type: "captureDetached" });
        applyVoiceEvent({ type: "finalizeCompleted" });
      }
      return;
    }
    let stoppedBeforeRestart: Promise<void> | null = null;
    try {
      const sender = voiceAsrSenderRef.current;
      if (!sender) throw new Error("ASR session is not available");
      if (keepListening) {
        const commit = sender.enqueueCommit(reason);
        voiceAsrPacketCountRef.current = 0;
        const context = voiceContextRef.current;
        const nextSettings = effectiveCaptureSettings(
          voiceSettingsRef.current,
          voicePolicyRef.current,
        );
        if (context && nextSettings) {
          resetVoiceActivityDetector(voiceActivityDetectorRef, nextSettings, context.sampleRate);
        }
        await commit;
        applyVoiceEvent({ type: "captureStarted" });
        return;
      }
      const finalPacket = voiceAsrPacketizerRef.current.flushPadded();
      if (finalPacket) sender.enqueueAudio(finalPacket);
      const sessionId = voiceAsrSessionIdRef.current;
      if (listeningEnabledRef.current && sessionId) {
        stoppedBeforeRestart = waitForVoiceAsrStopped(sessionId);
      }
      applyVoiceEvent({ type: "captureDetached" });
      await detachVoiceCapture(false, false);
      await withTimeout(sender.enqueueStop(true), 3_000, "ASR stop timed out");
      return;
    } catch (cause) {
      if (!disposedRef.current) setError((current) => current ?? toMessage(cause));
    } finally {
      const pending = voiceSessionRef.current.pendingFinalize;
      applyVoiceEvent({ type: "finalizeCompleted" });
      if (
        suspensionReasonRef.current &&
        voiceSessionRef.current.capture === "recording"
      ) {
        await suspendVoice(suspensionReasonRef.current);
      }
      if (pending) {
        void finishVoiceCapture(pending === "continue");
      } else if (
        !keepListening &&
        listeningEnabledRef.current &&
        !conversationSessionRef.current.speechRunId
      ) {
        if (stoppedBeforeRestart) await stoppedBeforeRestart;
        if (
          disposedRef.current ||
          !listeningEnabledRef.current ||
          conversationSessionRef.current.speechRunId
        )
          return;
        void attachVoiceCapture();
      }
    }
  }

  function handleVoiceAsrEvent(event: VoiceAsrStreamEvent) {
    if (event.type === "stopped") {
      voiceAsrStopWaitersRef.current.get(event.sessionId)?.resolve();
    }
    if (!acceptedVoiceAsrSessionsRef.current.has(event.sessionId)) return;
    const ownsProjection =
      "utteranceId" in event && voiceAsrProjectionRef.current.utteranceId === event.utteranceId;
    const next = projectVoiceAsrEvent(voiceAsrProjectionRef.current, event);
    voiceAsrProjectionRef.current = next;
    setAsrProjection(next);
    if (event.type === "partial") setInterimTranscript(`${next.stableText}${next.unstableText}`);
    if (event.type === "utteranceDiscarded") setInterimTranscript("");
    if (event.type === "stopped") {
      acceptedVoiceAsrSessionsRef.current.delete(event.sessionId);
      voiceAsrConversationsRef.current.delete(event.sessionId);
    }
    if (event.type === "failed" && event.fatal) {
      setError((current) => current ?? voiceStartupMessage(event.code));
      void terminateFailedVoiceCapture();
    }
    if (event.type !== "final" || disposedRef.current) return;
    if (ownsProjection) setInterimTranscript(event.text);
    const conversationId = voiceAsrConversationsRef.current.get(event.sessionId) ?? null;
    if (!conversationId) return;
    const result = voiceFinalDeliveryRef.current.push({
      sessionId: event.sessionId,
      utteranceId: event.utteranceId,
      conversationId,
      text: event.text,
    });
    if (result === "full") {
      auditVoiceDeliveryBlocked(event.sessionId, event.utteranceId, conversationId);
      setError((current) => current ?? uiMessage("chatVoicePendingLimit"));
      void terminateFailedVoiceCapture();
      return;
    }
    if (result !== "accepted") return;
    const queued = voiceFinalDeliveryRef.current.claim(event.utteranceId);
    if (!queued) return;
    const onSettled = auditVoiceDeliverySettlement(queued, (delivered) =>
      voiceFinalDeliveryRef.current.settle(queued.utteranceId, delivered),
    );
    auditVoiceDeliveryDecision(queued, "immediate");
    pendingVoiceDeliveriesRef.current += 1;
    // LFM is intentionally bypassed: every finalized utterance enters the same Qwen turn path
    // as typed input. The conversation turn owner queues voice inputs while another run is active.
    void submitPrompt(queued.text, {
      inputOrigin: "voice",
      sourceId: queued.utteranceId,
      onSettled,
    })
      .catch((cause) => {
        onSettled(false);
        setError(`音声入力送信失敗: ${toMessage(cause)}`);
      })
      .finally(() => {
        pendingVoiceDeliveriesRef.current -= 1;
      });
  }

  async function terminateFailedVoiceCapture() {
    applyVoiceEvent({ type: "captureDetached" });
    await detachVoiceCapture(false);
  }

  return {
    attachVoiceCapture,
    suspendVoice,
    suspendVoiceForSpeech,
    resumeVoice,
    resumeVoiceAfterSpeech,
    finishVoiceCapture,
    handleVoiceAsrEvent,
    terminateFailedVoiceCapture,
  };
}
