import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import type { ConversationSession, PendingConversationPrompt, SubmitPromptOptions } from "../../lib/conversationSession";
import { resamplePcm } from "../../lib/audioResampling";
import { cancelRun } from "../../lib/runtime";
import type { VoiceActivityDetector } from "../../lib/voiceActivity";
import {
  initialVoiceSession,
  transitionVoiceSession,
  voiceCaptureState,
  voiceSessionBusy,
  voiceSessionProcessing,
  type VoiceSessionEvent,
} from "../../lib/voiceSession";
import { VoiceFrameBuffer } from "./voiceFrameBuffer";
import { type QueuedVoiceSegment, VoiceSegmentQueue } from "./voiceSegmentQueue";
import { attachAmbientVoiceCapture, resetVoiceActivityDetector } from "./ambientVoiceCapture";
import { drainVoiceSegmentQueue } from "./voiceSegmentProcessor";

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
  setRuntimeActivity: Dispatch<SetStateAction<string[]>>;
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
  const voiceFramesRef = useRef(new VoiceFrameBuffer());
  const voicePreRollFramesRef = useRef(new VoiceFrameBuffer());
  const voiceFlushResolverRef = useRef<(() => void) | null>(null);
  const voiceActivityDetectorRef = useRef<VoiceActivityDetector | null>(null);
  const voiceCaptureLeaseRef = useRef<(() => void) | null>(null);
  const voiceCaptureAttemptRef = useRef(0);
  const voiceSegmentQueueRef = useRef(new VoiceSegmentQueue());
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
      const transcriptionRunId = voiceSessionRef.current.transcriptionRunId;
      if (transcriptionRunId) void cancelRun(transcriptionRunId).catch(() => undefined);
      voiceFlushResolverRef.current?.();
      if (voiceNodeRef.current) voiceNodeRef.current.port.onmessage = null;
      voiceNodeRef.current?.disconnect();
      voiceSourceRef.current?.disconnect();
      voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
      void voiceContextRef.current?.close().catch(() => undefined);
      voiceCaptureLeaseRef.current?.();
      voiceCaptureLeaseRef.current = null;
      voiceFramesRef.current.clear();
      voicePreRollFramesRef.current.clear();
      voiceSegmentQueueRef.current.clear();
      pendingVoicePromptsRef.current = [];
    };
  }, [pendingVoicePromptsRef]);

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
        setError("Chat voice capture is disabled while a meeting is active or paused.");
        return;
      }
      if (!selectedConversationIdRef.current || !voiceSettingsRef.current) {
        setError("Voice settings are unavailable.");
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
    const capture = voiceSessionRef.current.capture;
    if (capture === "starting") {
      voiceFramesRef.current.clear();
      voicePreRollFramesRef.current.clear();
      applyVoiceEvent({ type: "captureDetached" });
      await detachVoiceCapture(false);
    } else if (capture === "recording") {
      await finishVoiceCapture(false);
    } else if (capture === "suspended") {
      applyVoiceEvent({ type: "captureDetached" });
    }
    const voiceRunId = voiceSessionRef.current.transcriptionRunId;
    voiceSegmentQueueRef.current.clear();
    if (voiceRunId) {
      applyVoiceEvent({ type: "transcriptionCancelRequested" });
      try {
        await cancelRun(voiceRunId);
      } catch (cause) {
        setError(toMessage(cause));
      }
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
      frames: voiceFramesRef,
      preRollFrames: voicePreRollFramesRef,
      flushResolver: voiceFlushResolverRef,
      activityDetector: voiceActivityDetectorRef,
      captureLease: voiceCaptureLeaseRef,
      applyEvent: applyVoiceEvent,
      finishSegment: () => void finishVoiceCapture(true),
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
    voiceFramesRef.current.clear();
    voicePreRollFramesRef.current.clear();
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

  async function detachVoiceCapture(flush: boolean) {
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
      const sampleRate = voiceContextRef.current?.sampleRate;
      if (!keepListening) {
        voiceActivityDetectorRef.current = null;
        applyVoiceEvent({ type: "captureDetached" });
        await detachVoiceCapture(true);
      }
      if (!sampleRate && voiceFramesRef.current.sampleCount === 0) {
        voicePreRollFramesRef.current.clear();
        return;
      }
      if (!sampleRate) throw new Error("Recorded audio is unavailable.");
      if (voiceFramesRef.current.sampleCount === 0) {
        voicePreRollFramesRef.current.clear();
        return;
      }
      const captured = voiceFramesRef.current.take();
      voicePreRollFramesRef.current.clear();
      let samples: Float32Array;
      try {
        samples = resamplePcm(captured, sampleRate, ASR_SAMPLE_RATE);
      } finally {
        captured.fill(0);
      }
      try {
        if (keepListening && voiceContextRef.current) {
          resetVoiceActivityDetector(voiceActivityDetectorRef, settings, voiceContextRef.current.sampleRate);
          applyVoiceEvent({ type: "captureStarted" });
        }
        enqueueVoiceSegment({
          conversationId,
          samples,
          sampleRate: ASR_SAMPLE_RATE,
          ttsActiveAtCapture: conversationSessionRef.current.speechRunId !== null,
        });
      } catch (cause) {
        samples.fill(0);
        throw cause;
      }
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

  function enqueueVoiceSegment(segment: QueuedVoiceSegment) {
    if (disposedRef.current) {
      segment.samples.fill(0);
      segment.samples = new Float32Array();
      return;
    }
    if (!voiceSegmentQueueRef.current.push(segment)) {
      setError("音声処理が追いつかないため、新しい発話は送信しませんでした。");
      return;
    }
    void drainVoiceSegments();
  }

  async function drainVoiceSegments() {
    await drainVoiceSegmentQueue({
      queue: voiceSegmentQueueRef.current,
      session: voiceSessionRef,
      disposed: disposedRef,
      conversation: conversationSessionRef,
      pendingPrompts: pendingVoicePromptsRef,
      applyEvent: applyVoiceEvent,
      setTranscript: setInterimTranscript,
      setError,
      setRuntimeActivity,
      stopSpeech,
      submitPrompt,
    });
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
