import type { AmbientVoiceSessionOptions } from "./ambientVoiceTypes";
import {
  effectiveCaptureSettings,
  voiceStartupMessage,
} from "./voiceCaptureSettings";
import { idleCaptureShouldStart } from "./idleVoiceCapture";
export { effectiveCaptureSettings } from "./voiceCaptureSettings";
import { VoiceCaptureResources } from "./VoiceCaptureResources";
import { useCommittedCallback } from "../../useCommittedCallback";
import { useLarmVoiceLifetime } from "./useLarmVoiceLifetime";
import { receiveLfmUtterance, speakLfmReply } from "../../lib/lfmConversationRuntime";
import { useEffect, useRef, useState } from "react";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../../lib/contracts";

import {
  appendVoiceAsrAudio,
  commitVoiceAsrUtterance,
  startVoiceAsrSession,
  stopVoiceAsrSession,
} from "../../lib/voiceAsrRuntime";
import {
  initialVoiceSession,
  transitionVoiceSession,
  voiceCaptureState,
  voiceSessionBusy,
  voiceSessionProcessing,
  type VoiceCaptureState,
  type VoiceSessionEvent,
} from "../../lib/voiceSession";
import { withTimeout } from "../../lib/promiseTimeout";
import { attachAmbientVoiceCapture, resetVoiceActivityDetector } from "./ambientVoiceCapture";
import { VoiceAsrPacketSender } from "./voiceAsrPacketSender";
import type { CommitReason } from "../../lib/generated/voiceAsr";
import { initialVoiceAsrProjection, projectVoiceAsrEvent } from "./voiceAsrProjection";
import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";
import {
  microphoneCaptureConstraints,
  MicrophoneCaptureError,
  requestMicrophoneStream,
} from "../../lib/microphone";
import {
  auditCaptureCancelled,
  auditCaptureFailed,
  auditCaptureStarted,
  auditCaptureSuspended,
  auditVoiceDeliveryBlocked,
  auditVoiceDeliveryDecision,
  auditVoiceDeliverySettlement,
} from "./voiceAudit";

const ASR_SAMPLE_RATE = 16_000;
export type { VoiceCaptureState } from "../../lib/voiceSession";
export type AmbientVoiceAvailability = VoiceCaptureState;
type SuspensionReason = "speech";

export function useAmbientVoiceSession({
  selectedConversationId,
  voiceSettings,
  voicePolicy,
  conversationSessionRef,
  pendingVoicePromptsRef,
  setError,
  stopSpeech,
  submitPrompt,
  persistListeningEnabled,
}: AmbientVoiceSessionOptions) {
  const [voiceSession, setVoiceSession] = useState(initialVoiceSession);
  const lfmInputQueueRef = useRef<Promise<void>>(Promise.resolve());
  const pendingLfmInputsRef = useRef(0);
  const [listeningEnabled, setListeningEnabled] = useState(false);
  const updateLarmLifetime = useLarmVoiceLifetime(
    listeningEnabled,
    selectedConversationId,
    () =>
      voiceSessionProcessing(voiceSessionRef.current) ||
      acceptedVoiceAsrSessionsRef.current.size > 0 ||
      !!conversationSessionRef.current.runId ||
      pendingVoicePromptsRef.current.length > 0 ||
      pendingLfmInputsRef.current > 0,
    (message) => setError((current) => current ?? message),
  );
  const [interimTranscript, setInterimTranscript] = useState("");
  const [asrProjection, setAsrProjection] = useState(initialVoiceAsrProjection);
  const [voiceActivityLevel, setVoiceActivityLevel] = useState(0);
  const [voiceActivityDetected, setVoiceActivityDetected] = useState(false);
  const voiceActivityLevelRef = useRef(0);
  const voiceActivityDetectedRef = useRef(false);
  const voiceActivityUpdatedAtRef = useRef(0);
  const voiceSessionRef = useRef(initialVoiceSession);
  const voiceToggleGenerationRef = useRef(0);
  const suspensionReasonRef = useRef<SuspensionReason | null>(null);
  const speechResumeTokenRef = useRef<string | null>(null);
  const [resources] = useState(() => new VoiceCaptureResources());
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
    disposedRef,
    listeningEnabledRef,
    detachVoiceCapture,
    waitForVoiceAsrStopped,
    packetVoiceFrame,
  } = resources;
  const previousInputDeviceIdRef = useRef<string | null>(null);
  const previousConversationIdRef = useRef<string | null>(null);
  const selectedConversationIdRef = useRef<string | null>(null);
  const voiceSettingsRef = useRef<VoiceSettings | null>(null);
  const voicePolicyRef = useRef<ConversationVoicePolicySnapshot | null>(null);
  selectedConversationIdRef.current = selectedConversationId;
  voiceSettingsRef.current = voiceSettings;
  voicePolicyRef.current = voicePolicy;

  function applyVoiceEvent(event: VoiceSessionEvent) {
    const next = transitionVoiceSession(voiceSessionRef.current, event);
    voiceSessionRef.current = next;
    if (!disposedRef.current) setVoiceSession(next);
    return next;
  }

  const voiceState = voiceCaptureState(voiceSession, listeningEnabled);
  const voiceAvailability = voiceState;

  useEffect(() => {
    if (voiceState === "listening") return;
    voiceActivityLevelRef.current = 0;
    voiceActivityDetectedRef.current = false;
    setVoiceActivityLevel(0);
    setVoiceActivityDetected(false);
  }, [voiceState]);

  const updateListeningEnabledCommitted = useCommittedCallback(updateListeningEnabled);
  const pauseAmbientCaptureCommitted = useCommittedCallback(pauseAmbientCapture);
  const restartCaptureForInputDeviceChangeCommitted = useCommittedCallback(
    restartCaptureForInputDeviceChange,
  );
  const restartCaptureForConversationChangeCommitted = useCommittedCallback(
    restartCaptureForConversationChange,
  );
  const attachVoiceCaptureCommitted = useCommittedCallback(attachVoiceCapture);
  useEffect(() => {
    const enabled = voiceSettings?.listeningEnabled ?? false;
    updateListeningEnabledCommitted(enabled);
    if (!enabled) void pauseAmbientCaptureCommitted(false);
  }, [
    voiceSettings?.listeningEnabled,
    updateListeningEnabledCommitted,
    pauseAmbientCaptureCommitted,
  ]);

  useEffect(() => {
    const inputDeviceId = voiceSettings?.inputDeviceId ?? null;
    const previousInputDeviceId = previousInputDeviceIdRef.current;
    previousInputDeviceIdRef.current = inputDeviceId;
    if (!inputDeviceId || !previousInputDeviceId || inputDeviceId === previousInputDeviceId) return;
    void restartCaptureForInputDeviceChangeCommitted(inputDeviceId);
  }, [voiceSettings?.inputDeviceId, restartCaptureForInputDeviceChangeCommitted]);

  useEffect(() => {
    const detector = voiceActivityDetectorRef.current;
    const context = voiceContextRef.current;
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (!detector || !context || !settings || detector.hasDetectedSpeech()) return;
    resetVoiceActivityDetector(voiceActivityDetectorRef, settings, context.sampleRate);
  }, [
    voicePolicy?.conversationId,
    voicePolicy?.policyRevision,
    voiceActivityDetectorRef,
    voiceContextRef,
  ]);

  useEffect(() => {
    disposedRef.current = false;

    return () => {
      disposedRef.current = true;
      resources.dispose();
      pendingVoicePromptsRef.current = [];
    };
  }, [pendingVoicePromptsRef, resources, disposedRef]);

  useEffect(() => {
    const previousConversationId = previousConversationIdRef.current;
    previousConversationIdRef.current = selectedConversationId;
    voiceFinalDeliveryRef.current.clear();
    voiceAsrProjectionRef.current = initialVoiceAsrProjection;
    setAsrProjection(initialVoiceAsrProjection);
    setInterimTranscript("");
    if (previousConversationId && previousConversationId !== selectedConversationId) {
      pendingVoicePromptsRef.current = [];
      acceptedVoiceAsrSessionsRef.current.clear();
      voiceAsrConversationsRef.current.clear();
      void restartCaptureForConversationChangeCommitted();
    }
  }, [
    selectedConversationId,
    pendingVoicePromptsRef,
    restartCaptureForConversationChangeCommitted,
    voiceAsrConversationsRef,
    acceptedVoiceAsrSessionsRef,
    voiceFinalDeliveryRef,
    voiceAsrProjectionRef,
  ]);

  useEffect(() => {
    if (
      !idleCaptureShouldStart({
        listeningEnabled,
        selectedConversationId,
        voiceSettings,
        situationHold: voicePolicy?.speechReasonCode === "situation_hold",
        speechRunId: conversationSessionRef.current.speechRunId,
        capture: voiceSessionRef.current.capture,
        hasStream: Boolean(voiceStreamRef.current),
      })
    )
      return;
    void attachVoiceCaptureCommitted();
  }, [
    listeningEnabled,
    selectedConversationId,
    voicePolicy?.speechReasonCode,
    voiceSettings,
    conversationSessionRef,
    attachVoiceCaptureCommitted,
    voiceStreamRef,
  ]);

  function updateListeningEnabled(enabled: boolean) {
    listeningEnabledRef.current = enabled;
    updateLarmLifetime(enabled);
    setListeningEnabled(enabled);
  }

  async function toggleAmbientListening(requestedEnabled?: boolean) {
    const generation = ++voiceToggleGenerationRef.current;
    const currentUiState = voiceCaptureState(
      voiceSessionRef.current,
      listeningEnabledRef.current,
    );
    const shouldEnable = requestedEnabled ?? currentUiState === "stopped";
    if (!shouldEnable) {
      // Stopping is always accepted. Keep the UI in preparing until owned
      // microphone resources have actually been released.
      updateListeningEnabled(false);
      try {
        await pauseAmbientCapture(true);
      } catch (cause) {
        if (generation === voiceToggleGenerationRef.current) setError(voiceStartupMessage(cause));
      }
      return;
    }
    if (voiceSessionRef.current.actionInProgress) return;
    updateListeningEnabled(true);
    applyVoiceEvent({ type: "actionStarted" });
    let persistedEnabled = false;
    try {
      setError(null);
      if (!selectedConversationIdRef.current || !voiceSettingsRef.current) {
        setError(uiMessage("chatVoiceSettingsUnavailable"));
        updateListeningEnabled(false);
        return;
      }
      if (voiceSessionRef.current.finalizing) return;
      if (conversationSessionRef.current.speechRunId) {
        await stopSpeech();
        if (
          generation !== voiceToggleGenerationRef.current ||
          !listeningEnabledRef.current ||
          conversationSessionRef.current.speechRunId
        )
          return;
      }
      const permissionStream = await requestMicrophoneStream(
        microphoneCaptureConstraints(voiceSettingsRef.current.inputDeviceId),
      );
      permissionStream.getTracks().forEach((track) => track.stop());
      if (generation !== voiceToggleGenerationRef.current || !listeningEnabledRef.current) return;
      await persistListeningEnabled(true);
      persistedEnabled = true;
      if (generation !== voiceToggleGenerationRef.current || !listeningEnabledRef.current) {
        if (listeningEnabledRef.current) return;
        await persistListeningEnabled(false).catch(() => undefined);
        return;
      }
      await attachVoiceCapture();
    } catch (cause) {
      if (generation === voiceToggleGenerationRef.current && listeningEnabledRef.current) {
        updateListeningEnabled(false);
        if (persistedEnabled) await persistListeningEnabled(false).catch(() => undefined);
        setError(voiceStartupMessage(cause));
      }
    } finally {
      applyVoiceEvent({ type: "actionFinished" });
    }
  }

  async function pauseAmbientCapture(persist: boolean) {
    updateListeningEnabled(false);
    setInterimTranscript("");
    suspensionReasonRef.current = null;
    speechResumeTokenRef.current = null;
    let persistenceFailure: unknown = null;
    const persistence = persist
      ? persistListeningEnabled(false).catch((cause) => {
          persistenceFailure = cause;
        })
      : Promise.resolve();
    // Pausing ambient listening only prevents future capture. Audio that has
    // already been finalized must still be transcribed and delivered.
    const capture = voiceSessionRef.current.capture;
    if (capture === "starting") {
      await detachVoiceCapture(false);
      applyVoiceEvent({ type: "captureDetached" });
    } else if (capture === "recording") {
      await finishVoiceCapture(false);
    } else if (capture === "suspended") {
      applyVoiceEvent({ type: "captureDetached" });
    }
    await persistence;
    if (persistenceFailure) throw persistenceFailure;
  }

  async function restartCaptureForInputDeviceChange(inputDeviceId: string) {
    if (!listeningEnabledRef.current) return;
    const capture = voiceSessionRef.current.capture;
    if (capture === "recording") {
      await finishVoiceCapture(false);
      return;
    }
    if (capture === "starting") {
      applyVoiceEvent({ type: "captureDetached" });
      await detachVoiceCapture(false);
    }
    if (
      disposedRef.current ||
      !listeningEnabledRef.current ||
      voiceSettingsRef.current?.inputDeviceId !== inputDeviceId ||
      conversationSessionRef.current.speechRunId
    )
      return;
    await attachVoiceCapture();
  }

  async function restartCaptureForConversationChange() {
    applyVoiceEvent({ type: "captureDetached" });
    await detachVoiceCapture(false);
    if (
      disposedRef.current ||
      !listeningEnabledRef.current ||
      !selectedConversationIdRef.current ||
      conversationSessionRef.current.speechRunId
    )
      return;
    await attachVoiceCapture();
  }

  async function attachVoiceCapture() {
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (
      disposedRef.current ||
      !settings ||
      !selectedConversationIdRef.current ||
      voiceStreamRef.current
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
      if (cause instanceof MicrophoneCaptureError) {
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
    if (!voiceStreamRef.current && voiceSessionRef.current.capture !== "starting") return false;
    suspensionReasonRef.current = reason;
    auditCaptureSuspended(voiceAsrSessionIdRef.current, selectedConversationIdRef.current, reason);
    applyVoiceEvent({ type: "captureSuspended" });
    await detachVoiceCapture(false);
    return true;
  }

  async function suspendVoiceForSpeech(speechRunId: string): Promise<boolean> {
    speechResumeTokenRef.current = speechRunId;
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
    pendingLfmInputsRef.current += 1;
    // Serialize LFM inputs only. Never wait for or cancel Qwen to accept the next utterance.
    lfmInputQueueRef.current = lfmInputQueueRef.current.then(async () => {
      if (disposedRef.current || selectedConversationIdRef.current !== conversationId) {
        onSettled(false);
        pendingLfmInputsRef.current -= 1;
        return;
      }
      try {
        const decision = await receiveLfmUtterance(conversationId, queued.utteranceId, queued.text);
        onSettled(true);
        if (disposedRef.current || selectedConversationIdRef.current !== conversationId) return;
        void speakLfmReply(conversationId, queued.utteranceId, decision.speechEpoch, (message) =>
          setError(message),
        ).catch((cause) => setError(`LFM 音声: ${toMessage(cause)}`));
        if (decision.reasoningRequestId && decision.requestContent) {
          void submitPrompt(decision.requestContent, {
            inputOrigin: "voice",
            sourceId: decision.reasoningRequestId,
          });
        }
      } catch (cause) {
        onSettled(false);
        setError(`LFM 応対失敗: ${toMessage(cause)}`);
      } finally {
        pendingLfmInputsRef.current -= 1;
      }
    });
  }

  async function terminateFailedVoiceCapture() {
    applyVoiceEvent({ type: "captureDetached" });
    await detachVoiceCapture(false);
  }

  return {
    listeningEnabled,
    voiceActionInProgress: voiceSession.actionInProgress,
    voiceAvailability,
    voiceState,
    voiceBusy: voiceSessionBusy(voiceSession),
    voiceProcessing: voiceSessionProcessing(voiceSession),
    voiceActivityLevel,
    voiceActivityDetected,
    interimTranscript: { text: interimTranscript, projection: asrProjection },
    toggleAmbientListening,
    suspendVoiceForSpeech,
    resumeVoiceAfterSpeech,
  };
}
