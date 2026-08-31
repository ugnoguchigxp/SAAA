import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";
import { appendConversationActivity } from "../../lib/conversationActivity";
import type { ConversationVoicePolicySnapshot, MeetingState, VoiceSettings } from "../../lib/contracts";
import type { ConversationSession, PendingConversationPrompt, SubmitPromptOptions } from "../../lib/conversationSession";
import { appendVoiceAsrAudio, commitVoiceAsrUtterance, startVoiceAsrSession, stopVoiceAsrSession } from "../../lib/voiceAsrRuntime";
import type { VoiceActivityDetector } from "../../lib/voiceActivity";
import {
  initialVoiceSession,
  transitionVoiceSession,
  voiceCaptureState,
  voiceSessionBusy,
  voiceSessionProcessing,
  type VoiceSessionEvent,
} from "../../lib/voiceSession";
import { attachAmbientVoiceCapture, resetVoiceActivityDetector } from "./ambientVoiceCapture";
import { VoiceAsrPacketizer } from "./voiceAsrPacketizer";
import { VoiceAsrPacketSender } from "./voiceAsrPacketSender";
import type { CommitReason } from "./voiceAsrPacketSender";
import { initialVoiceAsrProjection, projectVoiceAsrEvent } from "./voiceAsrProjection";
import { VoiceFinalDeliveryQueue } from "./voiceFinalDeliveryQueue";
import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";
import {
  microphoneCaptureConstraints,
  microphoneErrorMessage,
  MicrophoneCaptureError,
  requestMicrophoneStream,
} from "../../lib/microphone";

const ASR_SAMPLE_RATE = 16_000;
export type VoiceCaptureState = "idle" | "recording" | "transcribing";
export type AmbientVoiceAvailability = "disabled" | "connecting" | "listening" | "suspended" | "blocked";
type SuspensionReason = "speech" | "meeting";
type VoiceAsrStopWaiter = { promise: Promise<void>; resolve: () => void };

function voiceStartupMessage(cause: unknown): string {
  if (cause instanceof MicrophoneCaptureError) return microphoneErrorMessage(cause);
  switch (toMessage(cause)) {
    case "asr-provider-unavailable":
      return uiMessage("chatVoiceAsrUnavailable");
    case "asr-session-exists":
      return uiMessage("chatVoiceSessionConflict");
    case "asr-target-speaker-unavailable":
      return uiMessage("chatVoiceTargetSpeakerModeUnavailable");
    default:
      return uiMessage("chatVoiceCaptureInitializationFailed");
  }
}

export function useAmbientVoiceSession({
  selectedConversationId,
  voiceSettings,
  voicePolicy,
  meetingState,
  conversationSessionRef,
  pendingVoicePromptsRef,
  setError,
  setRuntimeActivity,
  stopSpeech,
  submitPrompt,
  persistListeningEnabled,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  voicePolicy: ConversationVoicePolicySnapshot | null;
  meetingState: MeetingState;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  setError: Dispatch<SetStateAction<string | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<ConversationRuntimeActivity[]>>;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
  persistListeningEnabled: (enabled: boolean) => Promise<void>;
}) {
  const [voiceSession, setVoiceSession] = useState(initialVoiceSession);
  const [listeningEnabled, setListeningEnabled] = useState(false);
  const [interimTranscript, setInterimTranscript] = useState("");
  const [asrProjection, setAsrProjection] = useState(initialVoiceAsrProjection);
  const voiceSessionRef = useRef(initialVoiceSession);
  const listeningEnabledRef = useRef(false);
  const suspensionReasonRef = useRef<SuspensionReason | null>(null);
  const speechResumeTokenRef = useRef<string | null>(null);
  const voiceStreamRef = useRef<MediaStream | null>(null);
  const voiceContextRef = useRef<AudioContext | null>(null);
  const voiceSourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const voiceNodeRef = useRef<AudioWorkletNode | null>(null);
  const voiceFlushResolverRef = useRef<(() => void) | null>(null);
  const voiceActivityDetectorRef = useRef<VoiceActivityDetector | null>(null);
  const voiceCaptureLeaseRef = useRef<(() => void) | null>(null);
  const voiceCaptureAttemptRef = useRef(0);
  const voiceAsrPacketizerRef = useRef(new VoiceAsrPacketizer());
  const voiceAsrSenderRef = useRef<VoiceAsrPacketSender | null>(null);
  const voiceAsrSessionIdRef = useRef<string | null>(null);
  const acceptedVoiceAsrSessionsRef = useRef(new Set<string>());
  const voiceAsrConversationsRef = useRef(new Map<string, string>());
  const voiceAsrStopWaitersRef = useRef(new Map<string, VoiceAsrStopWaiter>());
  const voiceAsrPacketCountRef = useRef(0);
  const voiceAsrProjectionRef = useRef(initialVoiceAsrProjection);
  const voiceFinalDeliveryRef = useRef(new VoiceFinalDeliveryQueue());
  const previousInputDeviceIdRef = useRef<string | null>(null);
  const previousConversationIdRef = useRef<string | null>(null);
  const disposedRef = useRef(false);
  const meetingStateRef = useRef<MeetingState>("idle");
  const selectedConversationIdRef = useRef<string | null>(null);
  const voiceSettingsRef = useRef<VoiceSettings | null>(null);
  const voicePolicyRef = useRef<ConversationVoicePolicySnapshot | null>(null);
  meetingStateRef.current = meetingState;
  selectedConversationIdRef.current = selectedConversationId;
  voiceSettingsRef.current = voiceSettings;
  voicePolicyRef.current = voicePolicy;

  function applyVoiceEvent(event: VoiceSessionEvent) {
    const next = transitionVoiceSession(voiceSessionRef.current, event);
    voiceSessionRef.current = next;
    if (!disposedRef.current) setVoiceSession(next);
    return next;
  }

  const voiceState: VoiceCaptureState = voiceCaptureState(voiceSession);
  const voiceStarting = voiceSession.capture === "starting";
  const voiceAvailability: AmbientVoiceAvailability = !listeningEnabled
    ? "disabled"
    : voiceSession.capture === "starting"
      ? "connecting"
      : voiceSession.capture === "recording"
        ? "listening"
        : voiceSession.capture === "suspended"
          ? "suspended"
          : "blocked";

  useEffect(() => {
    const enabled = voiceSettings?.listeningEnabled ?? false;
    updateListeningEnabled(enabled);
    if (!enabled) void pauseAmbientCapture(false);
  }, [voiceSettings?.listeningEnabled]);

  useEffect(() => {
    const inputDeviceId = voiceSettings?.inputDeviceId ?? null;
    const previousInputDeviceId = previousInputDeviceIdRef.current;
    previousInputDeviceIdRef.current = inputDeviceId;
    if (!inputDeviceId || !previousInputDeviceId || inputDeviceId === previousInputDeviceId) return;
    void restartCaptureForInputDeviceChange(inputDeviceId);
  }, [voiceSettings?.inputDeviceId]);

  useEffect(() => {
    const detector = voiceActivityDetectorRef.current;
    const context = voiceContextRef.current;
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (!detector || !context || !settings || detector.hasDetectedSpeech()) return;
    resetVoiceActivityDetector(voiceActivityDetectorRef, settings, context.sampleRate);
  }, [voicePolicy?.conversationId, voicePolicy?.policyRevision]);

  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      voiceCaptureAttemptRef.current += 1;
      voiceFlushResolverRef.current?.();
      if (voiceNodeRef.current) voiceNodeRef.current.port.onmessage = null;
      voiceNodeRef.current?.disconnect();
      voiceSourceRef.current?.disconnect();
      voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
      void voiceContextRef.current?.close().catch(() => undefined);
      voiceCaptureLeaseRef.current?.();
      voiceCaptureLeaseRef.current = null;
      const sender = voiceAsrSenderRef.current;
      voiceAsrSenderRef.current = null;
      const sessionId = voiceAsrSessionIdRef.current;
      voiceAsrSessionIdRef.current = null;
      if (sender) {
        void sender.enqueueStop(false).catch(() => sessionId
          ? stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined)
          : undefined);
      }
      else if (sessionId) void stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
      voiceFinalDeliveryRef.current.clear();
      voiceAsrStopWaitersRef.current.forEach((waiter) => waiter.resolve());
      voiceAsrStopWaitersRef.current.clear();
      acceptedVoiceAsrSessionsRef.current.clear();
      voiceAsrConversationsRef.current.clear();
      pendingVoicePromptsRef.current = [];
    };
  }, [pendingVoicePromptsRef]);

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
      void restartCaptureForConversationChange();
    }
  }, [selectedConversationId]);

  useEffect(() => {
    if (
      !listeningEnabled
      || !selectedConversationId
      || !voiceSettings
      || isMeetingBlocking(meetingState)
      || conversationSessionRef.current.speechRunId
      || voiceSessionRef.current.capture !== "idle"
      || voiceStreamRef.current
    ) return;
    void attachVoiceCapture();
  }, [listeningEnabled, meetingState, selectedConversationId, voiceSettings]);

  useEffect(() => {
    if (!isMeetingBlocking(meetingState) && suspensionReasonRef.current === "meeting") {
      void resumeVoiceAfterMeeting();
    }
  }, [meetingState]);

  function updateListeningEnabled(enabled: boolean) {
    listeningEnabledRef.current = enabled;
    setListeningEnabled(enabled);
  }

  async function toggleAmbientListening(requestedEnabled?: boolean) {
    if (voiceSessionRef.current.actionInProgress) return;
    applyVoiceEvent({ type: "actionStarted" });
    try {
      setError(null);
      const capture = voiceSessionRef.current.capture;
      if (requestedEnabled === false) {
        if (listeningEnabledRef.current || capture !== "idle") await pauseAmbientCapture(true);
        return;
      }
      if (capture === "starting" || capture === "recording" || capture === "suspended") {
        await pauseAmbientCapture(true);
        return;
      }
      if (listeningEnabledRef.current && voiceSessionProcessing(voiceSessionRef.current)) {
        await pauseAmbientCapture(true);
        return;
      }
      if (listeningEnabledRef.current) {
        await attachVoiceCapture();
        return;
      }
      if (isMeetingBlocking(meetingStateRef.current)) {
        setError(uiMessage("chatVoiceBlockedDuringMeeting"));
        return;
      }
      if (!selectedConversationIdRef.current || !voiceSettingsRef.current) {
        setError(uiMessage("chatVoiceSettingsUnavailable"));
        return;
      }
      if (conversationSessionRef.current.speechRunId) {
        await stopSpeech();
        if (conversationSessionRef.current.speechRunId) return;
      }
      const permissionStream = await requestMicrophoneStream(
        microphoneCaptureConstraints(voiceSettingsRef.current.inputDeviceId),
      );
      permissionStream.getTracks().forEach((track) => track.stop());
      await persistListeningEnabled(true);
      updateListeningEnabled(true);
      await attachVoiceCapture();
    } catch (cause) {
      setError(voiceStartupMessage(cause));
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
    if (persist) {
      try {
        await persistListeningEnabled(false);
      } catch (cause) {
        persistenceFailure = cause;
      }
    }
    // Pausing ambient listening only prevents future capture. Audio that has
    // already been finalized must still be transcribed and delivered.
    const capture = voiceSessionRef.current.capture;
    if (capture === "starting") {
      applyVoiceEvent({ type: "captureDetached" });
      await detachVoiceCapture(false);
    } else if (capture === "recording") {
      await finishVoiceCapture(false);
    } else if (capture === "suspended") {
      applyVoiceEvent({ type: "captureDetached" });
    }
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
      disposedRef.current
      || !listeningEnabledRef.current
      || voiceSettingsRef.current?.inputDeviceId !== inputDeviceId
      || isMeetingBlocking(meetingStateRef.current)
      || conversationSessionRef.current.speechRunId
    ) return;
    await attachVoiceCapture();
  }

  async function restartCaptureForConversationChange() {
    applyVoiceEvent({ type: "captureDetached" });
    await detachVoiceCapture(false);
    if (
      disposedRef.current
      || !listeningEnabledRef.current
      || !selectedConversationIdRef.current
      || isMeetingBlocking(meetingStateRef.current)
      || conversationSessionRef.current.speechRunId
    ) return;
    await attachVoiceCapture();
  }

  async function attachVoiceCapture() {
    const settings = effectiveCaptureSettings(voiceSettingsRef.current, voicePolicyRef.current);
    if (disposedRef.current || !settings || !selectedConversationIdRef.current || voiceStreamRef.current) return;
    if (voiceSessionRef.current.capture === "starting" || voiceSessionRef.current.capture === "recording") return;
    const sessionId = crypto.randomUUID();
    voiceAsrSessionIdRef.current = sessionId;
    acceptedVoiceAsrSessionsRef.current.add(sessionId);
    voiceAsrConversationsRef.current.set(sessionId, selectedConversationIdRef.current);
    applyVoiceEvent({ type: "captureStarting" });
    try {
      const start = (recoverExisting: boolean) => startVoiceAsrSession({
        sessionId,
        conversationId: selectedConversationIdRef.current!,
        sampleRate: ASR_SAMPLE_RATE,
        recoverExisting,
      }, handleVoiceAsrEvent);
      try {
        await start(false);
      } catch (cause) {
        if (toMessage(cause) !== "asr-session-exists") throw cause;
        await start(true);
      }
      if (
        disposedRef.current
        || voiceAsrSessionIdRef.current !== sessionId
        || !acceptedVoiceAsrSessionsRef.current.has(sessionId)
      ) {
        await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
        return;
      }
      voiceAsrPacketizerRef.current.reset();
      voiceAsrPacketCountRef.current = 0;
      voiceAsrSenderRef.current = new VoiceAsrPacketSender({
        append: (sequence, bytes) => appendVoiceAsrAudio(sessionId, sequence, bytes),
        commit: (reason) => commitVoiceAsrUtterance({ sessionId, reason }),
        stop: (finalizeCurrent) => stopVoiceAsrSession({ sessionId, finalizeCurrent }),
      }, (error) => {
        setError((current) => current ?? error.message);
        void terminateFailedVoiceCapture();
      });
      await attachAmbientVoiceCapture({
        settings,
        disposed: disposedRef,
        listeningEnabled: listeningEnabledRef,
        meetingState: meetingStateRef,
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
      });
    } catch (cause) {
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
    applyVoiceEvent({ type: "captureSuspended" });
    await detachVoiceCapture(false);
    return true;
  }

  async function suspendVoiceForSpeech(speechRunId: string): Promise<boolean> {
    speechResumeTokenRef.current = speechRunId;
    return suspendVoice("speech");
  }

  async function suspendVoiceForMeeting(): Promise<void> {
    speechResumeTokenRef.current = null;
    await suspendVoice("meeting");
  }

  async function resumeVoice(reason: SuspensionReason): Promise<void> {
    if (disposedRef.current || suspensionReasonRef.current !== reason) return;
    if (!listeningEnabledRef.current) {
      suspensionReasonRef.current = null;
      applyVoiceEvent({ type: "captureDetached" });
      return;
    }
    if (isMeetingBlocking(meetingStateRef.current)) {
      suspensionReasonRef.current = "meeting";
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
    if (
      listeningEnabledRef.current
      && voiceSessionRef.current.capture === "idle"
      && !isMeetingBlocking(meetingStateRef.current)
    ) await attachVoiceCapture();
  }

  async function resumeVoiceAfterMeeting(): Promise<void> {
    await resumeVoice("meeting");
  }

  async function detachVoiceCapture(flush: boolean, stopSession = true) {
    voiceCaptureAttemptRef.current += 1;
    if (flush && voiceNodeRef.current) {
      await new Promise<void>((resolve) => {
        let completed = false;
        const finish = () => {
          if (completed) return;
          completed = true;
          voiceFlushResolverRef.current = null;
          window.clearTimeout(timeout);
          resolve();
        };
        const timeout = window.setTimeout(finish, 250);
        voiceFlushResolverRef.current = finish;
        voiceNodeRef.current?.port.postMessage({ type: "flush" });
      });
    }
    if (voiceNodeRef.current) voiceNodeRef.current.port.onmessage = null;
    voiceNodeRef.current?.disconnect();
    voiceSourceRef.current?.disconnect();
    voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
    voiceNodeRef.current = null;
    voiceSourceRef.current = null;
    voiceStreamRef.current = null;
    if (voiceContextRef.current) await voiceContextRef.current.close().catch(() => undefined);
    voiceContextRef.current = null;
    voiceActivityDetectorRef.current = null;
    voiceCaptureLeaseRef.current?.();
    voiceCaptureLeaseRef.current = null;
    const sender = voiceAsrSenderRef.current;
    voiceAsrSenderRef.current = null;
    voiceAsrPacketizerRef.current.reset();
    const sessionId = voiceAsrSessionIdRef.current;
    voiceAsrSessionIdRef.current = null;
    if (sender && stopSession) {
      const stopped = await sender.enqueueStop(false).then(() => true, () => false);
      if (!stopped && sessionId) {
        await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
      }
    } else if (sessionId && stopSession) {
      await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
    }
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
      await sender.enqueueStop(true);
      await detachVoiceCapture(false, false);
      return;
    } catch (cause) {
      if (!disposedRef.current) setError((current) => current ?? toMessage(cause));
    } finally {
      const pending = voiceSessionRef.current.pendingFinalize;
      applyVoiceEvent({ type: "finalizeCompleted" });
      if (pending) {
        void finishVoiceCapture(pending === "continue");
      } else if (
        !keepListening
        && listeningEnabledRef.current
        && !isMeetingBlocking(meetingStateRef.current)
        && !conversationSessionRef.current.speechRunId
      ) {
        if (stoppedBeforeRestart) await stoppedBeforeRestart;
        if (
          disposedRef.current
          || !listeningEnabledRef.current
          || isMeetingBlocking(meetingStateRef.current)
          || conversationSessionRef.current.speechRunId
        ) return;
        void attachVoiceCapture();
      }
    }
  }

  function packetVoiceFrame(frame: Float32Array) {
    const sender = voiceAsrSenderRef.current;
    if (!sender || disposedRef.current || !listeningEnabledRef.current) return;
    const packets = voiceAsrPacketizerRef.current.append(frame);
    for (const [index, packet] of packets.entries()) {
      try {
        sender.enqueueAudio(packet);
        voiceAsrPacketCountRef.current += 1;
      } catch {
        for (const unsent of packets.slice(index)) unsent.fill(0);
        break;
      }
    }
  }

  function handleVoiceAsrEvent(event: VoiceAsrStreamEvent) {
    if (event.type === "stopped") {
      voiceAsrStopWaitersRef.current.get(event.sessionId)?.resolve();
    }
    if (!acceptedVoiceAsrSessionsRef.current.has(event.sessionId)) return;
    const ownsProjection = "utteranceId" in event
      && voiceAsrProjectionRef.current.utteranceId === event.utteranceId;
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
    const result = voiceFinalDeliveryRef.current.push({ sessionId: event.sessionId, utteranceId: event.utteranceId, conversationId, text: event.text });
    if (result === "full") {
      setError((current) => current ?? uiMessage("chatVoicePendingLimit"));
      void terminateFailedVoiceCapture();
      return;
    }
    if (result !== "accepted") return;
    const queueBehindActiveTurn = Boolean(conversationSessionRef.current.runId);
    if (queueBehindActiveTurn && pendingVoicePromptsRef.current.length >= 2) {
      setError((current) => current ?? uiMessage("chatVoicePendingLimit"));
      void terminateFailedVoiceCapture();
      return;
    }
    const queued = voiceFinalDeliveryRef.current.claim(event.utteranceId);
    if (!queued) return;
    const onSettled = (delivered: boolean) => voiceFinalDeliveryRef.current.settle(queued.utteranceId, delivered);
    if (queueBehindActiveTurn) {
      pendingVoicePromptsRef.current.push({ content: queued.text, inputOrigin: "voice", sourceId: queued.utteranceId, onSettled });
      setRuntimeActivity((current) => appendConversationActivity(current, { type: "voiceQueryQueued" }));
      return;
    }
    void submitPrompt(queued.text, { inputOrigin: "voice", sourceId: queued.utteranceId, onSettled });
  }

  async function terminateFailedVoiceCapture() {
    applyVoiceEvent({ type: "captureDetached" });
    await detachVoiceCapture(false);
  }

  function waitForVoiceAsrStopped(sessionId: string): Promise<void> {
    const existing = voiceAsrStopWaitersRef.current.get(sessionId);
    if (existing) return existing.promise;
    let complete!: () => void;
    const promise = new Promise<void>((resolve) => {
      const timeout = window.setTimeout(() => complete(), 17_000);
      complete = () => {
        window.clearTimeout(timeout);
        voiceAsrStopWaitersRef.current.delete(sessionId);
        resolve();
      };
    });
    voiceAsrStopWaitersRef.current.set(sessionId, { promise, resolve: complete });
    return promise;
  }

  return {
    listeningEnabled,
    voiceActionInProgress: voiceSession.actionInProgress,
    voiceStarting,
    voiceAvailability,
    voiceState,
    voiceBusy: voiceSessionBusy(voiceSession),
    voiceProcessing: voiceSessionProcessing(voiceSession),
    interimTranscript: { text: interimTranscript, projection: asrProjection },
    toggleAmbientListening,
    suspendVoiceForMeeting,
    suspendVoiceForSpeech,
    resumeVoiceAfterSpeech,
  };
}

export function effectiveCaptureSettings(
  settings: VoiceSettings | null,
  policy: ConversationVoicePolicySnapshot | null,
): VoiceSettings | null {
  if (!settings) return null;
  return policy
    ? { ...settings, silenceTimeoutMs: policy.effectiveSilenceTimeoutMs }
    : settings;
}
