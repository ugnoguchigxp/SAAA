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
  auditVoiceDeliveryBlocked,
  auditVoiceDeliveryDecision,
  auditVoiceDeliverySettlement,
} from "./voiceAudit";
import type { VoiceCaptureResources } from "./VoiceCaptureResources";

const ASR_SAMPLE_RATE = 16_000;
// Enable only after live playback-only and overlapped-speech acceptance succeeds.
const SELF_VOICE_EXCLUSION_VERIFIED = false;

export function canAcceptSpeechDuringPlayback(
  nativeCapture: boolean,
  bargeInEnabled: boolean | undefined,
  scope: "all-speakers" | "target-speaker" | null,
): boolean {
  return SELF_VOICE_EXCLUSION_VERIFIED && nativeCapture && bargeInEnabled !== false && scope === "target-speaker";
}

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
    speechSuppressedUtteranceIdsRef,
    bargeInSpeechSinceRef,
    bargeInFiredForRef,
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
    speechResumeTokenRef,
    voiceActivityLevelRef,
    voiceActivityDetectedRef,
    voiceActivityUpdatedAtRef,
    pendingVoiceDeliveriesRef,
    conversationSessionRef,
    ttsStartedAtRef,
    stopSpeech,
  } = input;
  const bargeInSpeechSince = bargeInSpeechSinceRef;
  const bargeInFiredFor = bargeInFiredForRef;

  async function attachVoiceCapture() {
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (
      disposedRef.current ||
      !settings ||
      !selectedConversationIdRef.current ||
      voiceStreamRef.current ||
      nativeCaptureRef.current
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
        finishSegment: (reason) => {
          if (reason === "silence" && voiceAsrProjectionRef.current.scope !== "all-speakers")
            return;
          void finishVoiceCapture(true, reason);
        },
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
        bargeInEnabled: SELF_VOICE_EXCLUSION_VERIFIED && settings.bargeInEnabled,
        speechIsPlaying: () => Boolean(conversationSessionRef.current.speechRunId),
        speechRunId: () => conversationSessionRef.current.speechRunId,
        bargeInSpeechSince,
        bargeInFiredFor,
        ttsStartedAtMs: () => ttsStartedAtRef.current,
        interruptSpeech: () => {
          void interruptNativeVoicePlayback();
          void stopSpeech();
        },
        onNativeEnded: (message) => {
          setError((current) => current ?? message);
          void terminateFailedVoiceCapture();
        },
      });
      auditCaptureStarted(
        sessionId,
        selectedConversationIdRef.current,
        voiceSessionRef.current.capture,
      );
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

  async function suspendVoiceForSpeech(speechRunId: string): Promise<boolean> {
    speechResumeTokenRef.current = speechRunId;
    ttsStartedAtRef.current = performance.now();
    const projection = voiceAsrProjectionRef.current;
    if (!canAcceptSpeechDuringPlayback(nativeCaptureRef.current, voiceSettingsRef.current?.bargeInEnabled, projection.scope)) {
      if (projection.utteranceId) speechSuppressedUtteranceIdsRef.current.add(projection.utteranceId);
    }
    // Listening stays active during TTS. Unverified playback transcripts are filtered below.
    return Boolean(voiceStreamRef.current || nativeCaptureRef.current);
  }

  async function resumeVoiceAfterSpeech(speechRunId: string): Promise<void> {
    if (speechResumeTokenRef.current !== speechRunId) return;
    speechResumeTokenRef.current = null;
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
        const nextSettings = effectiveCaptureSettings(
          voiceSettingsRef.current,
          voicePolicyRef.current,
        );
        if (nextSettings) {
          // Native VoiceProcessing has no AudioContext. Reset anyway, or the detector
          // stays finalized and later speech never becomes a chat utterance.
          resetVoiceActivityDetector(
            voiceActivityDetectorRef,
            nextSettings,
            voiceContextRef.current?.sampleRate ?? ASR_SAMPLE_RATE,
          );
        }
        const tail = voiceAsrPacketizerRef.current.flushPadded();
        if (tail) {
          sender.enqueueAudio(tail);
          voiceAsrPacketCountRef.current += 1;
        }
        const commit = sender.enqueueCommit(reason);
        voiceAsrPacketCountRef.current = 0;
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
      if (pending) {
        void finishVoiceCapture(pending === "continue");
      } else if (
        !keepListening &&
        listeningEnabledRef.current &&
        selectedConversationIdRef.current === conversationId
      ) {
        if (stoppedBeforeRestart) await stoppedBeforeRestart;
        if (
          disposedRef.current ||
          !listeningEnabledRef.current ||
          selectedConversationIdRef.current !== conversationId
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
    if (event.type === "ready" || event.type === "stopped") {
      speechSuppressedUtteranceIdsRef.current.clear();
    }
    if (event.type === "ready" && conversationSessionRef.current.speechRunId &&
      !canAcceptSpeechDuringPlayback(nativeCaptureRef.current, voiceSettingsRef.current?.bargeInEnabled, event.scope)) {
      speechSuppressedUtteranceIdsRef.current.add(event.currentUtteranceId);
    }
    if ("utteranceId" in event && event.utteranceId && conversationSessionRef.current.speechRunId &&
      !canAcceptSpeechDuringPlayback(nativeCaptureRef.current, voiceSettingsRef.current?.bargeInEnabled, voiceAsrProjectionRef.current.scope)) {
      speechSuppressedUtteranceIdsRef.current.add(event.utteranceId);
    }
    const ownsProjection =
      "utteranceId" in event && voiceAsrProjectionRef.current.utteranceId === event.utteranceId;
    const next = projectVoiceAsrEvent(voiceAsrProjectionRef.current, event);
    voiceAsrProjectionRef.current = next;
    setAsrProjection(next);
    if (
      next.scope === "target-speaker" &&
      (event.type === "final" || event.type === "utteranceDiscarded")
    ) {
      const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
      if (settings) {
        resetVoiceActivityDetector(
          voiceActivityDetectorRef,
          settings,
          voiceContextRef.current?.sampleRate ?? ASR_SAMPLE_RATE,
        );
      }
      voiceAsrPacketCountRef.current = 0;
      voiceActivityDetectedRef.current = false;
      setVoiceActivityDetected(false);
    }
    if (event.type === "partial") setInterimTranscript(`${next.stableText}${next.unstableText}`);
    if (event.type === "utteranceDiscarded") {
      speechSuppressedUtteranceIdsRef.current.delete(event.utteranceId);
      setInterimTranscript("");
      if (event.reason === "target-speaker-empty")
        setError((current) => current ?? uiMessage("voiceTargetSpeakerRejected"));
    }
    if (event.type === "stopped") {
      acceptedVoiceAsrSessionsRef.current.delete(event.sessionId);
      voiceAsrConversationsRef.current.delete(event.sessionId);
    }
    if (event.type === "failed" && event.fatal) {
      setError((current) => current ?? voiceStartupMessage(event.code));
      void terminateFailedVoiceCapture();
    }
    if (event.type !== "final" || disposedRef.current) return;
    if (speechSuppressedUtteranceIdsRef.current.delete(event.utteranceId)) return;
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
    suspendVoiceForSpeech,
    resumeVoiceAfterSpeech,
    finishVoiceCapture,
    handleVoiceAsrEvent,
    terminateFailedVoiceCapture,
  };
}
