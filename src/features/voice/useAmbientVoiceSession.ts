import {
  createAmbientVoiceCaptureActions,
  type SuspensionReason,
} from "./ambientVoiceCaptureActions";
import type { AmbientVoiceSessionOptions } from "./ambientVoiceTypes";
import { effectiveCaptureSettings, voiceStartupMessage } from "./voiceCaptureSettings";
import { idleCaptureShouldStart } from "./idleVoiceCapture";
export { effectiveCaptureSettings } from "./voiceCaptureSettings";
import { VoiceCaptureResources } from "./VoiceCaptureResources";
import { useCommittedCallback } from "../../useCommittedCallback";
import { useLarmVoiceLifetime } from "./useLarmVoiceLifetime";
import { useEffect, useRef, useState } from "react";
import { uiMessage } from "../../i18n/presentation";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../../lib/contracts";

import {
  initialVoiceSession,
  transitionVoiceSession,
  voiceCaptureState,
  voiceSessionBusy,
  voiceSessionProcessing,
  type VoiceCaptureState,
  type VoiceSessionEvent,
} from "../../lib/voiceSession";
import { resetVoiceActivityDetector } from "./ambientVoiceCapture";
import { initialVoiceAsrProjection } from "./voiceAsrProjection";
import { microphoneCaptureConstraints, requestMicrophoneStream } from "../../lib/microphone";
import { audioBackendStatus, nativeCapturePreferred } from "../../lib/audioBackend";
import { withTimeout } from "../../lib/promiseTimeout";
import {
  auditVoicePreflightFailed,
  auditVoiceStartBlocked,
  auditVoiceToggleRequested,
} from "./voiceAudit";

export type { VoiceCaptureState } from "../../lib/voiceSession";
export type AmbientVoiceAvailability = VoiceCaptureState;

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
  const pendingVoiceDeliveriesRef = useRef(0);
  const [listeningEnabled, setListeningEnabled] = useState(false);
  const updateLarmLifetime = useLarmVoiceLifetime(
    listeningEnabled,
    selectedConversationId,
    () =>
      voiceSessionProcessing(voiceSessionRef.current) ||
      acceptedVoiceAsrSessionsRef.current.size > 0 ||
      !!conversationSessionRef.current.runId ||
      pendingVoicePromptsRef.current.length > 0 ||
      pendingVoiceDeliveriesRef.current > 0,
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
  const ttsStartedAtRef = useRef(0);
  const [resources] = useState(() => new VoiceCaptureResources());
  const {
    voiceStreamRef,
    voiceContextRef,
    voiceActivityDetectorRef,
    acceptedVoiceAsrSessionsRef,
    voiceAsrConversationsRef,
    voiceAsrProjectionRef,
    voiceFinalDeliveryRef,
    nativeCaptureRef,
    disposedRef,
    listeningEnabledRef,
    detachVoiceCapture,
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
  const { attachVoiceCapture, suspendVoiceForSpeech, resumeVoiceAfterSpeech, finishVoiceCapture } =
    createAmbientVoiceCaptureActions({
      resources,
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
    });
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
        hasStream: Boolean(voiceStreamRef.current || nativeCaptureRef.current),
        actionInProgress: voiceSession.actionInProgress,
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
    voiceSession.actionInProgress,
  ]);

  function updateListeningEnabled(enabled: boolean) {
    listeningEnabledRef.current = enabled;
    updateLarmLifetime(enabled);
    setListeningEnabled(enabled);
  }

  async function toggleAmbientListening(requestedEnabled?: boolean) {
    const currentUiState = voiceCaptureState(voiceSessionRef.current, listeningEnabledRef.current);
    const shouldEnable = requestedEnabled ?? currentUiState === "stopped";
    auditVoiceToggleRequested(selectedConversationIdRef.current, shouldEnable, currentUiState);
    if (shouldEnable && voiceSessionRef.current.actionInProgress) {
      auditVoiceStartBlocked(selectedConversationIdRef.current, "action-in-progress");
      return;
    }
    const generation = ++voiceToggleGenerationRef.current;
    if (!shouldEnable) {
      // Stopping is always accepted. Keep the UI in preparing until owned
      // microphone resources have actually been released.
      updateListeningEnabled(false);
      try {
        await pauseAmbientCapture(true);
      } catch (cause) {
        if (generation === voiceToggleGenerationRef.current) setError(voiceStartupMessage(cause));
      } finally {
        if (generation === voiceToggleGenerationRef.current)
          applyVoiceEvent({ type: "actionFinished" });
      }
      return;
    }
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
      let skipGetUserMedia = false;
      try {
        const status = await audioBackendStatus();
        skipGetUserMedia = nativeCapturePreferred(
          status,
          voiceSettingsRef.current.aecEnabled,
        );
      } catch {
        skipGetUserMedia = false;
      }
      if (!skipGetUserMedia) {
        const permissionStream = await withTimeout(
          requestMicrophoneStream(
            microphoneCaptureConstraints(
              voiceSettingsRef.current.inputDeviceId,
              voiceSettingsRef.current.aecEnabled,
            ),
          ),
          30_000,
          "microphone-startup-timeout",
          (lateStream) => lateStream.getTracks().forEach((track) => track.stop()),
        );
        permissionStream.getTracks().forEach((track) => track.stop());
      }
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
        auditVoicePreflightFailed(selectedConversationIdRef.current, cause);
        updateListeningEnabled(false);
        if (persistedEnabled) await persistListeningEnabled(false).catch(() => undefined);
        setError(voiceStartupMessage(cause));
      }
    } finally {
      if (generation === voiceToggleGenerationRef.current)
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
