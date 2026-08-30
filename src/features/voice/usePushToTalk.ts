import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import type { ConversationSession, PendingConversationPrompt, SubmitPromptOptions } from "../../lib/conversationSession";
import {
  disposeMicrophoneCapture,
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  MicrophoneCaptureError,
  requestMicrophoneStream,
} from "../../lib/microphone";
import { resamplePcm } from "../../lib/audioResampling";
import { cancelRun, transcribeAudio } from "../../lib/runtime";
import { VoiceActivityDetector } from "../../lib/voiceActivity";
import {
  initialVoiceSession,
  transitionVoiceSession,
  voiceCaptureState,
  voiceSessionBusy,
  type VoiceSessionEvent,
} from "../../lib/voiceSession";
import { VoiceFrameBuffer } from "./voiceFrameBuffer";
import { type QueuedVoiceSegment, VoiceSegmentQueue } from "./voiceSegmentQueue";

const ASR_SAMPLE_RATE = 16_000;
const MAX_VOICE_SEGMENT_SECONDS = 30;
export type VoiceCaptureState = "idle" | "recording" | "transcribing";

export function usePushToTalk({
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
  const [interimTranscript, setInterimTranscript] = useState("");
  const voiceSessionRef = useRef(initialVoiceSession);
  const voiceStreamRef = useRef<MediaStream | null>(null);
  const voiceContextRef = useRef<AudioContext | null>(null);
  const voiceSourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const voiceNodeRef = useRef<AudioWorkletNode | null>(null);
  const voiceFramesRef = useRef(new VoiceFrameBuffer());
  const voiceFlushResolverRef = useRef<(() => void) | null>(null);
  const voiceActivityDetectorRef = useRef<VoiceActivityDetector | null>(null);
  const voiceCaptureLeaseRef = useRef<(() => void) | null>(null);
  const voiceCaptureAttemptRef = useRef(0);
  const voiceSegmentQueueRef = useRef(new VoiceSegmentQueue());
  const disposedRef = useRef(false);
  const meetingStateRef = useRef<MeetingState>("idle");
  meetingStateRef.current = meetingState;

  function applyVoiceEvent(event: VoiceSessionEvent) {
    const next = transitionVoiceSession(voiceSessionRef.current, event);
    voiceSessionRef.current = next;
    if (!disposedRef.current) setVoiceSession(next);
    return next;
  }

  const voiceState: VoiceCaptureState = voiceCaptureState(voiceSession);
  const voiceStarting = voiceSession.capture === "starting";

  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      voiceCaptureAttemptRef.current += 1;
      const transcriptionRunId = voiceSessionRef.current.transcriptionRunId;
      if (transcriptionRunId) void cancelRun(transcriptionRunId).catch(() => undefined);
      voiceFlushResolverRef.current?.();
      voiceNodeRef.current?.disconnect();
      voiceSourceRef.current?.disconnect();
      voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
      void voiceContextRef.current?.close().catch(() => undefined);
      voiceCaptureLeaseRef.current?.();
      voiceCaptureLeaseRef.current = null;
      voiceFramesRef.current.clear();
      voiceSegmentQueueRef.current.clear();
      pendingVoicePromptsRef.current = [];
    };
  }, [pendingVoicePromptsRef]);

  async function toggleVoiceCapture() {
    if (voiceSessionRef.current.actionInProgress) return;
    applyVoiceEvent({ type: "actionStarted" });
    try {
      setError(null);
      if (voiceSessionRef.current.capture === "suspended") {
        await stopSpeech();
        return;
      }
      if (voiceState === "recording") {
        void finishVoiceCapture(false);
        return;
      }
      const voiceRunId = voiceSessionRef.current.transcriptionRunId;
      if (voiceState === "transcribing" && voiceRunId) {
        applyVoiceEvent({ type: "transcriptionCancelRequested" });
        voiceSegmentQueueRef.current.clear();
        try {
          await cancelRun(voiceRunId);
        } catch (cause) {
          applyVoiceEvent({ type: "transcriptionFinished", runId: voiceRunId });
          setError(toMessage(cause));
        }
        return;
      }
      if (isMeetingBlocking(meetingStateRef.current)) {
        setError("Chat voice capture is disabled while a meeting is active or paused.");
        return;
      }
      if (!selectedConversationId || !voiceSettings) {
        setError("Voice settings are unavailable.");
        return;
      }
      if (voiceSettings.sttProviderId !== "network-asr" || voiceSettings.sttModel !== "qwen3-asr-1.7b") {
        setError("Voice settings must use the LAN ASR provider.");
        return;
      }
      if (conversationSessionRef.current.speechRunId) {
        await stopSpeech();
        if (conversationSessionRef.current.speechRunId) return;
      }
      await attachVoiceCapture();
    } finally {
      applyVoiceEvent({ type: "actionFinished" });
    }
  }

  async function attachVoiceCapture() {
    if (disposedRef.current || !voiceSettings || !selectedConversationId || voiceStreamRef.current) return;
    const captureAttempt = ++voiceCaptureAttemptRef.current;
    let stream: MediaStream | null = null;
    let context: AudioContext | null = null;
    try {
      applyVoiceEvent({ type: "captureStarting" });
      voiceCaptureLeaseRef.current = acquireAudioCapture("chat");
      const audio = microphoneCaptureConstraints(voiceSettings.inputDeviceId);
      stream = await requestMicrophoneStream(audio);
      if (disposedRef.current || voiceCaptureAttemptRef.current !== captureAttempt) { await disposeMicrophoneCapture(stream, null); return; }
      voiceStreamRef.current = stream;
      context = new AudioContext();
      const activeContext = context;
      voiceContextRef.current = activeContext;
      await activeContext.audioWorklet.addModule("/audio/meeting-processor.js");
      if (disposedRef.current || voiceCaptureAttemptRef.current !== captureAttempt) { await disposeMicrophoneCapture(stream, activeContext); return; }
      const source = activeContext.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(activeContext, "meeting-processor");
      voiceSourceRef.current = source;
      voiceNodeRef.current = node;
      voiceFramesRef.current.clear();
      voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: activeContext.sampleRate });
      node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
        if (!(event.data instanceof Float32Array)) {
          if (event.data.type === "flushed") voiceFlushResolverRef.current?.();
          return;
        }
        const observation = voiceActivityDetectorRef.current?.observe(event.data);
        voiceFramesRef.current.append(event.data);
        if (!observation?.hasSpeech) {
          const leadingSilenceLimit = Math.round(activeContext.sampleRate * 0.5);
          voiceFramesRef.current.trimStartTo(leadingSilenceLimit);
        }
        if (observation?.shouldFinalize || voiceFramesRef.current.sampleCount >= activeContext.sampleRate * MAX_VOICE_SEGMENT_SECONDS) {
          void finishVoiceCapture(true);
        }
      };
      source.connect(node);
      node.connect(activeContext.destination);
      await ensureMicrophoneAudioContextRunning(activeContext);
      if (disposedRef.current || voiceCaptureAttemptRef.current !== captureAttempt) { await disposeMicrophoneCapture(stream, activeContext); return; }
      setInterimTranscript("");
      applyVoiceEvent({ type: "captureStarted" });
    } catch (cause) {
      if (disposedRef.current || voiceCaptureAttemptRef.current !== captureAttempt) {
        await disposeMicrophoneCapture(stream, context);
        return;
      }
      await detachVoiceCapture(false);
      if (disposedRef.current) return;
      setError(cause instanceof MicrophoneCaptureError
        ? cause.message
        : `Voice capture initialization failed: ${toMessage(cause)}`);
      applyVoiceEvent({ type: "captureDetached" });
    }
  }

  async function suspendVoiceForSpeech(): Promise<boolean> {
    if (!voiceStreamRef.current && voiceSessionRef.current.capture !== "starting") return false;
    applyVoiceEvent({ type: "speechSuspended" });
    voiceFramesRef.current.clear();
    await detachVoiceCapture(false);
    return true;
  }

  async function resumeVoiceAfterSpeech(): Promise<void> {
    if (disposedRef.current || voiceSessionRef.current.capture !== "suspended") return;
    if (isMeetingBlocking(meetingStateRef.current)) {
      applyVoiceEvent({ type: "captureDetached" });
      return;
    }
    await attachVoiceCapture();
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
    if (!selectedConversationId || !voiceSettings) {
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
      if (!sampleRate) throw new Error("Recorded audio is unavailable.");
      if (voiceFramesRef.current.sampleCount === 0) return;
      const captured = voiceFramesRef.current.take();
      const samples = resamplePcm(captured, sampleRate, ASR_SAMPLE_RATE);
      captured.fill(0);
      if (keepListening && voiceContextRef.current) {
        voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: voiceContextRef.current.sampleRate });
        applyVoiceEvent({ type: "captureStarted" });
      }
      enqueueVoiceSegment({
        conversationId: selectedConversationId,
        model: voiceSettings.sttModel,
        samples,
        sampleRate: ASR_SAMPLE_RATE,
        ttsActiveAtCapture: conversationSessionRef.current.speechRunId !== null,
      });
    } catch (cause) {
      if (!disposedRef.current) setError((current) => current ?? toMessage(cause));
    } finally {
      const pending = voiceSessionRef.current.pendingFinalize;
      applyVoiceEvent({ type: "finalizeCompleted" });
      if (pending) void finishVoiceCapture(pending === "continue");
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
    if (voiceSessionRef.current.processingSegments) return;
    applyVoiceEvent({ type: "processingStarted" });
    try {
      while (voiceSegmentQueueRef.current.length > 0) {
        const segment = voiceSegmentQueueRef.current.shift();
        if (!segment) break;
        const runId = `voice_${crypto.randomUUID()}`;
        applyVoiceEvent({ type: "transcriptionStarted", runId });
        try {
          const transcript = await transcribeAudio({
            runId,
            conversationId: segment.conversationId,
            samples: segment.samples,
            sampleRate: segment.sampleRate,
            model: segment.model,
          }, (event) => {
            if (disposedRef.current || event.runId !== runId) return;
            if (event.type === "transcriptFinal") setInterimTranscript(event.text);
          });
          segment.samples.fill(0);
          segment.samples = new Float32Array();
          if (voiceSessionRef.current.cancellationRequested || !transcript.trim()) continue;
          if (disposedRef.current) continue;
          setInterimTranscript(transcript);
          if (conversationSessionRef.current.speechRunId) await stopSpeech();
          if (disposedRef.current) continue;
          if (conversationSessionRef.current.runId) {
            if (pendingVoicePromptsRef.current.length >= 2) {
              setError("応答待ちの音声クエリーが上限に達したため、新しい発話は送信しませんでした。");
            } else {
              pendingVoicePromptsRef.current.push({ content: transcript, inputOrigin: "voice" });
              setRuntimeActivity((current) => [...current, "Voice query queued until the active response completes"].slice(-8));
            }
          } else {
            void submitPrompt(transcript, { allowVoiceBusy: true, inputOrigin: "voice" });
          }
        } catch (cause) {
          segment.samples.fill(0);
          segment.samples = new Float32Array();
          const message = toMessage(cause);
          if (!voiceSessionRef.current.cancellationRequested && !(segment.ttsActiveAtCapture && message.startsWith("TARGET_SPEAKER_REJECTED")) && !disposedRef.current) {
            setError(message.startsWith("TARGET_SPEAKER_REJECTED") ? "登録した本人の声として確認できなかったため、文字起こしへ送信しませんでした。" : message);
          }
        } finally {
          applyVoiceEvent({ type: "transcriptionFinished", runId });
        }
      }
    } finally {
      applyVoiceEvent({ type: "processingFinished" });
    }
  }

  return {
    voiceStarting,
    voiceState,
    voiceBusy: voiceSessionBusy(voiceSession),
    interimTranscript,
    isBusy: () => voiceSessionBusy(voiceSessionRef.current),
    toggleVoiceCapture,
    suspendVoiceForSpeech,
    resumeVoiceAfterSpeech,
  };
}
