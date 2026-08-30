import { useEffect, useRef, useState } from "react";
import type { MeetingLane, MeetingSnapshot, MeetingState, VoiceSettings } from "../../lib/contracts";
import {
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  requestMicrophoneStream,
} from "../../lib/microphone";
import {
  appendMeetingAudioSegment,
  discardMeeting,
  getMeetingSnapshot,
  meetingPreflight,
  pauseMeeting,
  resumeMeeting,
  saveMeetingTranscript,
  startMeeting,
  stopMeeting,
  unwatchMeeting,
  watchMeeting,
} from "../../lib/runtime";
import { appendFrames } from "./audio/pcm";
import { SegmentQueue } from "./audio/segmentQueue";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import { resamplePcm } from "../../lib/audioResampling";

const MEETING_SAMPLE_RATE = 16_000;

const idle: MeetingSnapshot = {
  sessionId: null,
  state: "idle",
  captureToken: null,
  entries: 0,
  capabilities: { microphone: true, systemAudio: false, overlay: false, translation: false },
  error: null,
};

type Line = { sequence: number; lane: MeetingLane; text: string; language: string | null; partial?: boolean };
type PendingSegment = {
  sequence: number;
  samples: Float32Array;
  sampleRate: number;
  startedAtMs: number;
  durationMs: number;
};

export function useMeetingSession(
  voice: VoiceSettings | null,
  onStateChanged: (state: MeetingState) => void,
  setError: (value: string | null) => void,
) {
  const [snapshot, setSnapshot] = useState<MeetingSnapshot>(idle);
  const [transcript, setTranscript] = useState<Line[]>([]);
  const [elapsed, setElapsed] = useState(0);
  const [health, setHealth] = useState("idle");
  const [working, setWorking] = useState(false);
  const snapshotRef = useRef<MeetingSnapshot>(idle);
  const snapshotRevision = useRef(0);
  const stream = useRef<MediaStream | null>(null);
  const context = useRef<AudioContext | null>(null);
  const source = useRef<MediaStreamAudioSourceNode | null>(null);
  const node = useRef<AudioWorkletNode | null>(null);
  const flushResolver = useRef<(() => void) | null>(null);
  const frames = useRef<Float32Array[]>([]);
  const frameSamples = useRef(0);
  const frameStartedAtMs = useRef<number | null>(null);
  const queue = useRef(new SegmentQueue<PendingSegment>(2, (segment) => segment.samples.fill(0)));
  const processing = useRef<Promise<void> | null>(null);
  const sequence = useRef(0);
  const started = useRef(0);
  const operation = useRef(false);
  const captureLease = useRef<(() => void) | null>(null);

  function commitSnapshot(next: MeetingSnapshot) {
    snapshotRef.current = next;
    setSnapshot(next);
  }

  function applySnapshot(next: MeetingSnapshot) {
    snapshotRevision.current += 1;
    commitSnapshot(next);
  }

  async function refreshSnapshot(): Promise<MeetingSnapshot | null> {
    const revision = ++snapshotRevision.current;
    const next = await getMeetingSnapshot();
    if (revision !== snapshotRevision.current) return null;
    commitSnapshot(next);
    return next;
  }

  useEffect(() => {
    let cancelled = false;
    const subscriberId = `meeting_subscriber_${crypto.randomUUID().replace(/-/g, "")}`;
    void refreshSnapshot()
      .then((next) => {
        if (cancelled || !next) return;
        if (next.state === "active") setHealth("capture disconnected — pause or stop to recover");
      })
      .catch((cause) => {
        if (!cancelled) setError(toMessage(cause));
      });
    const watchRegistration = watchMeeting(subscriberId, (event) => {
      if (cancelled) return;
      if (event.type === "stateChanged") {
        void refreshSnapshot().catch((cause) => setError(toMessage(cause)));
      } else if (event.type === "transcriptPartial" || event.type === "transcriptFinal") {
        if (event.sessionId !== snapshotRef.current.sessionId) return;
        setTranscript((lines) => {
          const index = lines.findIndex((line) => line.sequence === event.sequence);
          const next = { sequence: event.sequence, lane: event.lane, text: event.text, language: event.language, partial: event.type === "transcriptPartial" };
          return index < 0 ? [...lines, next] : lines.map((line, lineIndex) => lineIndex === index ? next : line);
        });
      } else if (event.type === "failed") {
        if (event.sessionId && event.sessionId !== snapshotRef.current.sessionId) return;
        setError(`${event.code}: ${event.message} ${event.recovery}`);
        void refreshSnapshot().catch(() => undefined);
      }
    }).catch((cause) => { if (!cancelled) setError(toMessage(cause)); });
    return () => {
      cancelled = true;
      snapshotRevision.current += 1;
      void watchRegistration.then(() => unwatchMeeting(subscriberId)).catch(() => undefined);
      void detachCapture(true);
      onStateChanged("idle");
    };
  }, []);

  useEffect(() => {
    onStateChanged(snapshot.state);
  }, [onStateChanged, snapshot.state]);

  useEffect(() => {
    if (snapshot.state !== "active") return;
    const timer = window.setInterval(
      () => setElapsed(Math.floor((Date.now() - started.current) / 1_000)),
      1_000,
    );
    return () => window.clearInterval(timer);
  }, [snapshot.state]);

  async function detachCapture(clearPending: boolean) {
    if (clearPending) flushResolver.current?.();
    if (!clearPending && node.current) {
      await new Promise<void>((resolve) => {
        let completed = false;
        const finish = () => {
          if (completed) return;
          completed = true;
          flushResolver.current = null;
          window.clearTimeout(timeout);
          resolve();
        };
        const timeout = window.setTimeout(finish, 250);
        flushResolver.current = finish;
        node.current?.port.postMessage({ type: "flush" });
      });
    }
    node.current?.disconnect();
    source.current?.disconnect();
    stream.current?.getTracks().forEach((track) => track.stop());
    stream.current = null;
    node.current = null;
    source.current = null;
    if (context.current) await context.current.close().catch(() => undefined);
    context.current = null;
    captureLease.current?.();
    captureLease.current = null;
    if (clearPending) {
      for (const frame of frames.current) frame.fill(0);
      frames.current = [];
      frameSamples.current = 0;
      frameStartedAtMs.current = null;
      queue.current.clear();
    }
  }

  function enqueueBuffered(sampleRate: number, minimumDurationMs: number): boolean {
    if (frameSamples.current === 0) return false;
    const capturedStartedAtMs = frameStartedAtMs.current;
    const samples = appendFrames(new Float32Array(), frames.current);
    for (const frame of frames.current) frame.fill(0);
    frames.current = [];
    frameSamples.current = 0;
    frameStartedAtMs.current = null;
    const durationMs = Math.round((samples.length / sampleRate) * 1_000);
    if (durationMs < minimumDurationMs) {
      samples.fill(0);
      return false;
    }
    const segment: PendingSegment = {
      sequence: sequence.current,
      samples,
      sampleRate,
      startedAtMs: capturedStartedAtMs ?? Math.max(0, Date.now() - started.current - durationMs),
      durationMs,
    };
    if (!queue.current.push(segment)) {
      setHealth("degraded");
      setError("Meeting transcription cannot keep up. Capture was paused without evicting queued audio.");
      void pauseAfterFailure();
      return false;
    }
    sequence.current += 1;
    void drainQueue().catch(() => undefined);
    return true;
  }

  function drainQueue(): Promise<void> {
    if (processing.current) return processing.current;
    const task = (async () => {
      let segment: PendingSegment | undefined;
      while ((segment = queue.current.shift())) {
        const segmentSequence = segment.sequence;
        const currentSnapshot = snapshotRef.current;
        if (
          currentSnapshot.state !== "active" ||
          !currentSnapshot.sessionId ||
          !currentSnapshot.captureToken
        ) {
          segment.samples.fill(0);
          throw new Error("Meeting capture is no longer active.");
        }
        let normalized = new Float32Array();
        try {
          normalized = resamplePcm(segment.samples, segment.sampleRate, MEETING_SAMPLE_RATE);
          segment.samples.fill(0);
          segment.samples = new Float32Array();
          const result = await appendMeetingAudioSegment({
            sessionId: currentSnapshot.sessionId,
            captureToken: currentSnapshot.captureToken,
            lane: "microphone",
            sequence: segmentSequence,
            samples: normalized,
            sampleRate: MEETING_SAMPLE_RATE,
            startedAtMs: segment.startedAtMs,
            durationMs: segment.durationMs,
          });
          setTranscript((lines) => {
            const next = { sequence: segmentSequence, lane: "microphone" as const, text: result.text, language: result.language, partial: false };
            const index = lines.findIndex((line) => line.sequence === segmentSequence);
            return index < 0 ? [...lines, next] : lines.map((line, lineIndex) => lineIndex === index ? next : line);
          });
        } catch (cause) {
          const latest = await getMeetingSnapshot().catch(() => null);
          if (latest && ["idle", "paused", "completed"].includes(latest.state)) {
            applySnapshot(latest);
            throw cause;
          }
          setHealth("degraded");
          setError(toMessage(cause));
          await pauseAfterFailure();
          throw cause;
        } finally {
          normalized.fill(0);
        }
      }
    })();
    const tracked = task.finally(() => {
      if (processing.current === tracked) processing.current = null;
      if (queue.current.length > 0) void drainQueue().catch(() => undefined);
    });
    processing.current = tracked;
    return tracked;
  }

  async function pauseAfterFailure() {
    await detachCapture(true);
    try {
      const current = await getMeetingSnapshot();
      applySnapshot(current);
      if (current.state === "active" && current.sessionId) {
        applySnapshot(await pauseMeeting(current.sessionId));
      }
    } catch (cause) {
      setError(toMessage(cause));
    }
  }

  async function attachCapture() {
    if (!voice) throw new Error("Voice settings are unavailable.");
    if (!captureLease.current) captureLease.current = acquireAudioCapture("meeting");
    const device = microphoneCaptureConstraints(voice.inputDeviceId);
    const nextStream = await requestMicrophoneStream(device);
    stream.current = nextStream;
    try {
      const nextContext = new AudioContext();
      context.current = nextContext;
      await nextContext.audioWorklet.addModule("/audio/meeting-processor.js");
      const nextSource = nextContext.createMediaStreamSource(nextStream);
      const nextNode = new AudioWorkletNode(nextContext, "meeting-processor");
      source.current = nextSource;
      node.current = nextNode;
      nextNode.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
        if (!(event.data instanceof Float32Array)) {
          if (event.data.type === "flushed") flushResolver.current?.();
          return;
        }
        if (frameSamples.current === 0) {
          frameStartedAtMs.current = Math.max(0, Date.now() - started.current);
        }
        frames.current.push(event.data);
        frameSamples.current += event.data.length;
        if (frameSamples.current >= nextContext.sampleRate * 5) {
          enqueueBuffered(nextContext.sampleRate, 5_000);
        }
      };
      nextSource.connect(nextNode);
      nextNode.connect(nextContext.destination);
      await ensureMicrophoneAudioContextRunning(nextContext);
      setHealth("ready");
    } catch (cause) {
      await detachCapture(true);
      throw cause;
    }
  }

  function beginOperation(): boolean {
    if (operation.current) return false;
    operation.current = true;
    setWorking(true);
    return true;
  }

  function endOperation() {
    operation.current = false;
    setWorking(false);
  }

  async function start() {
    if (!voice || !beginOperation()) return;
    setError(null);
    applySnapshot({ ...snapshotRef.current, state: "preflight", error: null });
    let startedSession: string | null = null;
    try {
      captureLease.current = acquireAudioCapture("meeting");
      const check = await requestMicrophoneStream(microphoneCaptureConstraints(voice.inputDeviceId));
      check.getTracks().forEach((track) => track.stop());
      const preflight = await meetingPreflight({
        microphoneDeviceId: voice.inputDeviceId,
        systemAudioEnabled: false,
        sttModel: voice.sttModel,
        translationEnabled: false,
      });
      if (preflight.blockingErrors.length) {
        throw new Error(preflight.blockingErrors.map((entry) => entry.message).join(" "));
      }
      const next = await startMeeting({
        sessionId: `meeting_${crypto.randomUUID().replace(/-/g, "")}`,
        microphoneDeviceId: voice.inputDeviceId,
        microphoneEnabled: true,
        systemAudioEnabled: false,
        sttModel: voice.sttModel,
        translationEnabled: false,
        persistenceMode: "discard",
      });
      startedSession = next.sessionId;
      applySnapshot(next);
      started.current = Date.now();
      sequence.current = 0;
      setElapsed(0);
      setTranscript([]);
      await attachCapture();
    } catch (cause) {
      await detachCapture(true);
      if (startedSession) {
        await stopMeeting(startedSession).catch(() => undefined);
        await discardMeeting(startedSession).catch(() => undefined);
      }
      const restored = await getMeetingSnapshot().catch(() => null);
      applySnapshot(restored ?? idle);
      setError(`Meeting start failed: ${toMessage(cause)}`);
    } finally {
      endOperation();
    }
  }

  async function pause() {
    const current = snapshotRef.current;
    if (!current.sessionId || !beginOperation()) return;
    try {
      await detachCapture(true);
      applySnapshot(await pauseMeeting(current.sessionId));
      await processing.current?.catch(() => undefined);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endOperation();
    }
  }

  async function resume() {
    const current = snapshotRef.current;
    if (!current.sessionId || !beginOperation()) return;
    try {
      const next = await resumeMeeting(current.sessionId);
      applySnapshot(next);
      try {
        await attachCapture();
      } catch (cause) {
        applySnapshot(await pauseMeeting(current.sessionId));
        throw cause;
      }
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endOperation();
    }
  }

  async function stop() {
    const current = snapshotRef.current;
    if (!current.sessionId || !beginOperation()) return;
    try {
      const sampleRate = context.current?.sampleRate;
      setHealth("stopping");
      await detachCapture(false);
      if (sampleRate) enqueueBuffered(sampleRate, 1_000);
      await drainQueue().catch(() => undefined);
      queue.current.clear();
      applySnapshot(await stopMeeting(current.sessionId));
      setHealth("stopped");
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endOperation();
    }
  }

  async function save() {
    const current = snapshotRef.current;
    if (!current.sessionId || !beginOperation()) return;
    try {
      applySnapshot(await saveMeetingTranscript(current.sessionId));
      setTranscript([]);
      setHealth("idle");
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endOperation();
    }
  }

  async function discard() {
    const current = snapshotRef.current;
    if (!current.sessionId || !beginOperation()) return;
    try {
      await detachCapture(true);
      await discardMeeting(current.sessionId);
      applySnapshot(idle);
      setTranscript([]);
      setHealth("idle");
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endOperation();
    }
  }

  return { snapshot, transcript, elapsed, health, working, start, pause, resume, stop, save, discard };
}

function toMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
