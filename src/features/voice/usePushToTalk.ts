import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect, useRef, useState } from "react";
import { isMeetingBlocking, toMessage } from "../../lib/appHelpers";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import { ensureMicrophoneAudioContextRunning, MicrophoneCaptureError, requestMicrophoneStream } from "../../lib/microphone";
import { mergePcmFrames } from "../../lib/pcm";
import { cancelRun, previewAudio, transcribeAudio } from "../../lib/runtime";
import { VoiceActivityDetector } from "../../lib/voiceActivity";

type QueuedVoiceSegment = { conversationId: string; model: string; samples: number[]; sampleRate: number; ttsActiveAtCapture: boolean };

export type VoiceCaptureState = "idle" | "recording" | "transcribing";

export function usePushToTalk({
  selectedConversationId,
  voiceSettings,
  filterEnabled,
  meetingState,
  activeRunIdRef,
  activeTtsRunIdRef,
  streamingSpeechSessionRef,
  pendingVoicePromptsRef,
  setError,
  setRuntimeActivity,
  stopSpeech,
  submitPrompt,
}: {
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  filterEnabled: boolean;
  meetingState: MeetingState;
  activeRunIdRef: MutableRefObject<string | null>;
  activeTtsRunIdRef: MutableRefObject<string | null>;
  streamingSpeechSessionRef: MutableRefObject<unknown>;
  pendingVoicePromptsRef: MutableRefObject<string[]>;
  setError: Dispatch<SetStateAction<string | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<string[]>>;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, allowVoiceBusy?: boolean) => Promise<void>;
}) {
  const [voiceStarting, setVoiceStarting] = useState(false);
  const [voiceState, setVoiceState] = useState<VoiceCaptureState>("idle");
  const [interimTranscript, setInterimTranscript] = useState("");
  const voiceStreamRef = useRef<MediaStream | null>(null);
  const voiceContextRef = useRef<AudioContext | null>(null);
  const voiceSourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const voiceNodeRef = useRef<AudioWorkletNode | null>(null);
  const voiceFramesRef = useRef<Float32Array[]>([]);
  const voiceFrameSamplesRef = useRef(0);
  const voiceFlushResolverRef = useRef<(() => void) | null>(null);
  const voicePreviewStartedRef = useRef(false);
  const voicePreviewRunIdRef = useRef<string | null>(null);
  const voicePreviewPromiseRef = useRef<Promise<void> | null>(null);
  const voiceActionRef = useRef(false);
  const voiceFinalizingRef = useRef(false);
  const activeVoiceRunIdRef = useRef<string | null>(null);
  const voiceCancellationRequestedRef = useRef(false);
  const voiceActivityDetectorRef = useRef<VoiceActivityDetector | null>(null);
  const voiceCaptureLeaseRef = useRef<(() => void) | null>(null);
  const voiceSegmentQueueRef = useRef<QueuedVoiceSegment[]>([]);
  const voiceSegmentProcessingRef = useRef(false);
  const targetSpeakerFilterEnabledRef = useRef(false);
  const meetingStateRef = useRef<MeetingState>("idle");
  meetingStateRef.current = meetingState;
  targetSpeakerFilterEnabledRef.current = filterEnabled;

  useEffect(() => () => {
    const previewRunId = voicePreviewRunIdRef.current;
    if (previewRunId) void cancelRun(previewRunId);
    voiceNodeRef.current?.disconnect();
    voiceSourceRef.current?.disconnect();
    voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
    void voiceContextRef.current?.close();
    voiceCaptureLeaseRef.current?.();
    voiceCaptureLeaseRef.current = null;
    voiceFramesRef.current = [];
    voiceFrameSamplesRef.current = 0;
    for (const segment of voiceSegmentQueueRef.current) segment.samples = [];
    voiceSegmentQueueRef.current = [];
    pendingVoicePromptsRef.current = [];
  }, [pendingVoicePromptsRef]);

  async function toggleVoiceCapture() {
    if (voiceActionRef.current) return;
    voiceActionRef.current = true;
    try {
      if (voiceState === "recording") {
        void finishVoiceCapture(false);
        return;
      }
      const voiceRunId = activeVoiceRunIdRef.current;
      if (voiceState === "transcribing" && voiceRunId) {
        voiceCancellationRequestedRef.current = true;
        try {
          await cancelRun(voiceRunId);
        } catch (cause) {
          voiceCancellationRequestedRef.current = false;
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
      try {
        setError(null);
        setVoiceStarting(true);
        voiceCaptureLeaseRef.current = acquireAudioCapture("chat");
        const audio: MediaTrackConstraints = {
          autoGainControl: true,
          echoCancellation: true,
          noiseSuppression: true,
          ...(voiceSettings.inputDeviceId === "default"
            ? {}
            : { deviceId: { exact: voiceSettings.inputDeviceId } }),
        };
        const stream = await requestMicrophoneStream(audio);
        voiceStreamRef.current = stream;
        const context = new AudioContext();
        voiceContextRef.current = context;
        await context.audioWorklet.addModule("/audio/meeting-processor.js");
        const source = context.createMediaStreamSource(stream);
        const node = new AudioWorkletNode(context, "meeting-processor");
        voiceSourceRef.current = source;
        voiceNodeRef.current = node;
        voiceFramesRef.current = [];
        voiceFrameSamplesRef.current = 0;
        voicePreviewStartedRef.current = false;
        voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: context.sampleRate });
        node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
          if (!(event.data instanceof Float32Array)) {
            if (event.data.type === "flushed") voiceFlushResolverRef.current?.();
            return;
          }
          voiceFramesRef.current.push(event.data);
          voiceFrameSamplesRef.current += event.data.length;
          if (voiceActivityDetectorRef.current?.observe(event.data).shouldFinalize) {
            void finishVoiceCapture(targetSpeakerFilterEnabledRef.current);
            return;
          }
          if (!targetSpeakerFilterEnabledRef.current && !voicePreviewStartedRef.current && voiceFrameSamplesRef.current >= context.sampleRate * 2) {
            startVoicePreview(context.sampleRate);
          }
        };
        source.connect(node);
        node.connect(context.destination);
        await ensureMicrophoneAudioContextRunning(context);
        setInterimTranscript("");
        setVoiceState("recording");
      } catch (cause) {
        await detachVoiceCapture(false);
        setError(cause instanceof MicrophoneCaptureError
          ? cause.message
          : `Voice capture initialization failed: ${toMessage(cause)}`);
        setVoiceState("idle");
      }
    } finally {
      setVoiceStarting(false);
      voiceActionRef.current = false;
    }
  }

  function startVoicePreview(sampleRate: number) {
    if (!selectedConversationId || !voiceSettings || voicePreviewStartedRef.current || targetSpeakerFilterEnabledRef.current) return;
    voicePreviewStartedRef.current = true;
    const runId = `voice_preview_${crypto.randomUUID().replace(/-/g, "")}`;
    const samples = mergePcmFrames(voiceFramesRef.current, voiceFrameSamplesRef.current);
    voicePreviewRunIdRef.current = runId;
    const preview = previewAudio({
      runId,
      conversationId: selectedConversationId,
      samples: Array.from(samples),
      sampleRate,
      model: voiceSettings.sttModel,
    }, (event) => {
      if (event.runId === runId && event.type === "transcriptDelta") {
        setInterimTranscript(event.text);
      }
    }).then(() => undefined).catch(() => undefined).finally(() => {
      if (voicePreviewRunIdRef.current === runId) {
        voicePreviewRunIdRef.current = null;
        voicePreviewPromiseRef.current = null;
      }
    });
    voicePreviewPromiseRef.current = preview;
  }

  async function detachVoiceCapture(flush: boolean) {
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
    if (voiceFinalizingRef.current) return;
    voiceFinalizingRef.current = true;
    if (!selectedConversationId || !voiceSettings) {
      try {
        await detachVoiceCapture(false);
      } finally {
        setVoiceState("idle");
        voiceFinalizingRef.current = false;
      }
      return;
    }
    try {
      const sampleRate = voiceContextRef.current?.sampleRate;
      if (!keepListening) {
        voiceActivityDetectorRef.current = null;
        setVoiceState("transcribing");
        await detachVoiceCapture(true);
      }
      if (!sampleRate) throw new Error("Recorded audio is unavailable.");
      if (voiceFrameSamplesRef.current === 0) return;
      const samples = Array.from(mergePcmFrames(voiceFramesRef.current, voiceFrameSamplesRef.current));
      voiceFramesRef.current = [];
      voiceFrameSamplesRef.current = 0;
      voicePreviewStartedRef.current = false;
      if (keepListening && voiceContextRef.current) {
        voiceActivityDetectorRef.current = new VoiceActivityDetector({ sampleRate: voiceContextRef.current.sampleRate });
        setVoiceState("recording");
      }
      const previewRunId = voicePreviewRunIdRef.current;
      if (previewRunId) await cancelRun(previewRunId).catch(() => undefined);
      await voicePreviewPromiseRef.current?.catch(() => undefined);
      enqueueVoiceSegment({
        conversationId: selectedConversationId,
        model: voiceSettings.sttModel,
        samples,
        sampleRate,
        ttsActiveAtCapture: activeTtsRunIdRef.current !== null,
      });
    } catch (cause) {
      setError((current) => current ?? toMessage(cause));
    } finally {
      voiceFinalizingRef.current = false;
      if (!voiceStreamRef.current && !voiceSegmentProcessingRef.current) setVoiceState("idle");
    }
  }

  function enqueueVoiceSegment(segment: QueuedVoiceSegment) {
    if (voiceSegmentQueueRef.current.length >= 2) {
      setError("音声処理が追いつかないため、新しい発話は送信しませんでした。");
      return;
    }
    voiceSegmentQueueRef.current.push(segment);
    void drainVoiceSegments();
  }

  async function drainVoiceSegments() {
    if (voiceSegmentProcessingRef.current) return;
    voiceSegmentProcessingRef.current = true;
    try {
      while (voiceSegmentQueueRef.current.length > 0) {
        const segment = voiceSegmentQueueRef.current.shift();
        if (!segment) break;
        const runId = `voice_${crypto.randomUUID()}`;
        activeVoiceRunIdRef.current = runId;
        voiceCancellationRequestedRef.current = false;
        if (!voiceStreamRef.current) setVoiceState("transcribing");
        try {
          const transcript = await transcribeAudio({
            runId,
            conversationId: segment.conversationId,
            samples: segment.samples,
            sampleRate: segment.sampleRate,
            model: segment.model,
          }, (event) => {
            if (event.runId !== runId) return;
            if (event.type === "transcriptDelta" || event.type === "transcriptFinal") setInterimTranscript(event.text);
          });
          segment.samples = [];
          if (voiceCancellationRequestedRef.current || !transcript.trim()) continue;
          setInterimTranscript(transcript);
          if (activeTtsRunIdRef.current) await stopSpeech();
          else if (streamingSpeechSessionRef.current) await stopSpeech();
          if (activeRunIdRef.current) {
            if (pendingVoicePromptsRef.current.length >= 2) {
              setError("応答待ちの音声クエリーが上限に達したため、新しい発話は送信しませんでした。");
            } else {
              pendingVoicePromptsRef.current.push(transcript);
              setRuntimeActivity((current) => [...current, "Voice query queued until the active response completes"].slice(-8));
            }
          } else {
            void submitPrompt(transcript, true);
          }
        } catch (cause) {
          segment.samples = [];
          const message = toMessage(cause);
          if (!voiceCancellationRequestedRef.current && !(segment.ttsActiveAtCapture && message.startsWith("TARGET_SPEAKER_REJECTED"))) {
            setError(message.startsWith("TARGET_SPEAKER_REJECTED") ? "登録した本人の声として確認できなかったため、文字起こしへ送信しませんでした。" : message);
          }
        } finally {
          if (activeVoiceRunIdRef.current === runId) activeVoiceRunIdRef.current = null;
          voiceCancellationRequestedRef.current = false;
        }
      }
    } finally {
      voiceSegmentProcessingRef.current = false;
      setVoiceState(voiceStreamRef.current ? "recording" : "idle");
    }
  }

  return {
    voiceStarting,
    voiceState,
    voiceBusy: voiceStarting || voiceState !== "idle",
    interimTranscript,
    voiceActionRef,
    toggleVoiceCapture,
  };
}
