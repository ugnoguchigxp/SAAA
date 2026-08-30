import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import type { ConversationRuntimeActivity } from "../../lib/conversationActivity";
import { appendConversationActivity } from "../../lib/conversationActivity";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
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
import { initialVoiceAsrProjection, projectVoiceAsrEvent } from "./voiceAsrProjection";
import { VoiceFinalDeliveryQueue } from "./voiceFinalDeliveryQueue";
import type { VoiceAsrStreamEvent } from "../../lib/generated/voiceAsr";

const ASR_SAMPLE_RATE = 16_000;
export type VoiceCaptureState = "idle" | "recording" | "transcribing";
type SuspensionReason = "speech" | "meeting";

export function useAmbientVoiceSession({
  selectedConversationId,
  voiceSettings,
  meetingState,
  conversationSessionRef,
  pendingVoicePromptsRef,
  setError,
  setRuntimeActivity,
  stopSpeech,
  submitPrompt,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  meetingState: MeetingState;
  conversationSessionRef: MutableRefObject<ConversationSession>;
  pendingVoicePromptsRef: MutableRefObject<PendingConversationPrompt[]>;
  setError: Dispatch<SetStateAction<string | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<ConversationRuntimeActivity[]>>;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
}) {
  const [voiceSession, setVoiceSession] = useState(initialVoiceSession);
  const [listeningEnabled, setListeningEnabled] = useState(false);
  const [interimTranscript, setInterimTranscript] = useState("");
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
  const voiceAsrPacketCountRef = useRef(0);
  const voiceAsrProjectionRef = useRef(initialVoiceAsrProjection);
  const voiceFinalDeliveryRef = useRef(new VoiceFinalDeliveryQueue());
  const previousInputDeviceIdRef = useRef<string | null>(null);
  const disposedRef = useRef(false);
  const meetingStateRef = useRef<MeetingState>("idle");
  const selectedConversationIdRef = useRef<string | null>(null);
  const voiceSettingsRef = useRef<VoiceSettings | null>(null);
  meetingStateRef.current = meetingState;
  selectedConversationIdRef.current = selectedConversationId;
  voiceSettingsRef.current = voiceSettings;

  function applyVoiceEvent(event: VoiceSessionEvent) {
    const next = transitionVoiceSession(voiceSessionRef.current, event);
    voiceSessionRef.current = next;
    if (!disposedRef.current) setVoiceSession(next);
    return next;
  }

  const voiceState: VoiceCaptureState = voiceCaptureState(voiceSession);
  const voiceStarting = voiceSession.capture === "starting";

  useEffect(() => {
    const enabled = voiceSettings?.listeningEnabled ?? false;
    updateListeningEnabled(enabled);
    if (!enabled) void pauseAmbientCapture();
  }, [voiceSettings?.listeningEnabled]);

  useEffect(() => {
    const inputDeviceId = voiceSettings?.inputDeviceId ?? null;
    const previousInputDeviceId = previousInputDeviceIdRef.current;
    previousInputDeviceIdRef.current = inputDeviceId;
    if (!inputDeviceId || !previousInputDeviceId || inputDeviceId === previousInputDeviceId) return;
    void restartCaptureForInputDeviceChange(inputDeviceId);
  }, [voiceSettings?.inputDeviceId]);

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
      if (sender) void sender.enqueueStop(false).catch(() => undefined);
      else if (sessionId) void stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
      voiceFinalDeliveryRef.current.clear();
      pendingVoicePromptsRef.current = [];
    };
  }, [pendingVoicePromptsRef]);

  useEffect(() => {
    voiceFinalDeliveryRef.current.clear();
    voiceAsrProjectionRef.current = initialVoiceAsrProjection;
    setInterimTranscript("");
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

  async function toggleAmbientListening() {
    if (voiceSessionRef.current.actionInProgress) return;
    applyVoiceEvent({ type: "actionStarted" });
    try {
      setError(null);
      const capture = voiceSessionRef.current.capture;
      if (capture === "starting" || capture === "recording" || capture === "suspended") {
        await pauseAmbientCapture();
        return;
      }
      if (listeningEnabledRef.current && voiceSessionProcessing(voiceSessionRef.current)) {
        await pauseAmbientCapture();
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
      updateListeningEnabled(true);
      await attachVoiceCapture();
    } finally {
      applyVoiceEvent({ type: "actionFinished" });
    }
  }

  async function pauseAmbientCapture() {
    updateListeningEnabled(false);
    setInterimTranscript("");
    suspensionReasonRef.current = null;
    speechResumeTokenRef.current = null;
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

  async function attachVoiceCapture() {
    const settings = voiceSettingsRef.current;
    if (disposedRef.current || !settings || !selectedConversationIdRef.current || voiceStreamRef.current) return;
    if (voiceSessionRef.current.capture === "starting" || voiceSessionRef.current.capture === "recording") return;
    const sessionId = crypto.randomUUID();
    try {
      await startVoiceAsrSession({ sessionId, conversationId: selectedConversationIdRef.current, sampleRate: ASR_SAMPLE_RATE }, handleVoiceAsrEvent);
      voiceAsrSessionIdRef.current = sessionId;
      voiceAsrPacketizerRef.current.reset();
      voiceAsrPacketCountRef.current = 0;
      voiceAsrSenderRef.current = new VoiceAsrPacketSender({
        append: (sequence, bytes) => appendVoiceAsrAudio(sessionId, sequence, bytes),
        commit: (reason) => commitVoiceAsrUtterance({ sessionId, reason }),
        stop: (finalizeCurrent) => stopVoiceAsrSession({ sessionId, finalizeCurrent }),
      }, (error) => setError((current) => current ?? error.message));
    } catch (cause) { setError(toMessage(cause)); return; }
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
      finishSegment: () => void finishVoiceCapture(true),
      packetFrame: packetVoiceFrame,
      packetCount: () => voiceAsrPacketCountRef.current,
      clearTranscript: () => setInterimTranscript(""),
      setError,
    });
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
    voiceAsrSessionIdRef.current = null;
    if (sender && stopSession) await sender.enqueueStop(false).catch(() => undefined);
  }

  async function finishVoiceCapture(keepListening: boolean) {
    if (disposedRef.current) return;
    const mode = keepListening ? "continue" : "stop";
    if (voiceSessionRef.current.finalizing) {
      applyVoiceEvent({ type: "finalizeRequested", mode });
      return;
    }
    applyVoiceEvent({ type: "finalizeRequested", mode });
    const conversationId = selectedConversationIdRef.current;
    const settings = voiceSettingsRef.current;
    if (!conversationId || !settings) {
      try {
        await detachVoiceCapture(false);
      } finally {
        applyVoiceEvent({ type: "captureDetached" });
        applyVoiceEvent({ type: "finalizeCompleted" });
      }
      return;
    }
    try {
      const sender = voiceAsrSenderRef.current;
      if (!sender) throw new Error("ASR session is not available");
      if (keepListening) {
        await sender.enqueueCommit("silence");
        voiceAsrPacketCountRef.current = 0;
        const context = voiceContextRef.current;
        if (context) resetVoiceActivityDetector(voiceActivityDetectorRef, settings, context.sampleRate);
        applyVoiceEvent({ type: "captureStarted" });
        return;
      }
      const finalPacket = voiceAsrPacketizerRef.current.flushPadded();
      if (finalPacket) sender.enqueueAudio(finalPacket);
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
        void attachVoiceCapture();
      }
    }
  }

  function packetVoiceFrame(frame: Float32Array) {
    const sender = voiceAsrSenderRef.current;
    if (!sender || disposedRef.current || !listeningEnabledRef.current) return;
    for (const packet of voiceAsrPacketizerRef.current.append(frame)) { sender.enqueueAudio(packet); voiceAsrPacketCountRef.current += 1; }
  }

  function handleVoiceAsrEvent(event: VoiceAsrStreamEvent) {
    if (event.type !== "ready" && event.sessionId !== voiceAsrSessionIdRef.current) return;
    const next = projectVoiceAsrEvent(voiceAsrProjectionRef.current, event);
    voiceAsrProjectionRef.current = next;
    if (event.type === "partial") setInterimTranscript(`${next.stableText}${next.unstableText}`);
    if (event.type === "utteranceDiscarded") setInterimTranscript("");
    if (event.type === "failed" && event.fatal) setError((current) => current ?? event.message);
    if (event.type !== "final" || disposedRef.current) return;
    setInterimTranscript(event.text);
    const conversationId = selectedConversationIdRef.current;
    if (!conversationId) return;
    const result = voiceFinalDeliveryRef.current.push({ sessionId: event.sessionId, utteranceId: event.utteranceId, conversationId, text: event.text });
    if (result === "full") { setError((current) => current ?? uiMessage("chatVoicePendingLimit")); return; }
    if (result !== "accepted") return;
    const queued = voiceFinalDeliveryRef.current.shift();
    if (!queued) return;
    if (conversationSessionRef.current.runId) {
      if (pendingVoicePromptsRef.current.length >= 2) { setError((current) => current ?? uiMessage("chatVoicePendingLimit")); return; }
      pendingVoicePromptsRef.current.push({ content: queued.text, inputOrigin: "voice", sourceId: queued.utteranceId });
      setRuntimeActivity((current) => appendConversationActivity(current, { type: "voiceQueryQueued" }));
      return;
    }
    void submitPrompt(queued.text, { inputOrigin: "voice" });
  }

  return {
    listeningEnabled,
    voiceStarting,
    voiceState,
    voiceBusy: voiceSessionBusy(voiceSession),
    voiceProcessing: voiceSessionProcessing(voiceSession),
    interimTranscript,
    toggleAmbientListening,
    suspendVoiceForMeeting,
    suspendVoiceForSpeech,
    resumeVoiceAfterSpeech,
  };
}
