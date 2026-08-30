import type { VoiceEvent } from "../../lib/contracts";
import { resamplePcm } from "../../lib/audioResampling";
import { cancelRun, transcribeAudioChunk } from "../../lib/runtime";
import { VoiceFrameBuffer } from "./voiceFrameBuffer";

const ASR_SAMPLE_RATE = 16_000;
const STREAM_CHUNK_MS = 1_000;
const MAX_STREAM_CHUNK_MS = 5_000;

type ChunkInput = { runId: string; samples: Float32Array; sampleRate: number };
type ChunkRequest = (input: ChunkInput, onEvent: (event: VoiceEvent) => void) => Promise<string>;
type CancelRequest = (runId: string) => Promise<void>;
type ActiveChunk = { runId: string; started: Promise<void>; promise: Promise<void> };

export class AmbientVoiceTranscriber {
  private segmentId = 0;
  private readonly pending = new VoiceFrameBuffer();
  private sourceRate: number | null = null;
  private transcript = "";
  private active: ActiveChunk | null = null;
  private disposed = false;
  private onTranscript: (transcript: string) => void = () => {};
  private onError: (message: string) => void = () => {};

  constructor(
    private readonly requestChunk: ChunkRequest = transcribeAudioChunk,
    private readonly requestCancel: CancelRequest = cancelRun,
  ) {}

  activate(): void {
    this.disposed = false;
  }

  advanceSegment(): void {
    this.segmentId += 1;
    this.pending.clear();
    this.sourceRate = null;
    this.transcript = "";
  }

  append(
    frame: Float32Array,
    sourceRate: number,
    onTranscript: (transcript: string) => void,
    onError: (message: string) => void,
  ): void {
    if (this.disposed || frame.length === 0 || !Number.isFinite(sourceRate) || sourceRate <= 0) return;
    if (this.sourceRate !== null && this.sourceRate !== sourceRate) this.advanceSegment();
    this.sourceRate = sourceRate;
    this.onTranscript = onTranscript;
    this.onError = onError;
    this.pending.append(frame.slice());
    this.pump();
  }

  private pump(): void {
    const sourceRate = this.sourceRate;
    if (this.disposed || this.active || sourceRate === null) return;
    const minimumSamples = Math.ceil((STREAM_CHUNK_MS / 1_000) * sourceRate);
    if (this.pending.sampleCount < minimumSamples) return;
    const chunkSamples = Math.min(
      this.pending.sampleCount,
      Math.ceil((MAX_STREAM_CHUNK_MS / 1_000) * sourceRate),
    );
    const segmentId = this.segmentId;
    const captured = this.pending.takeStart(chunkSamples);
    let samples = new Float32Array();
    try {
      samples = resamplePcm(captured, sourceRate, ASR_SAMPLE_RATE);
    } finally {
      captured.fill(0);
    }
    const runId = `voice_chunk_${crypto.randomUUID()}`;
    let notifyStarted: () => void = () => {};
    const started = new Promise<void>((resolve) => { notifyStarted = resolve; });
    const promise = this.requestChunk({ runId, samples, sampleRate: ASR_SAMPLE_RATE }, (event) => {
      if (event.runId === runId && event.type === "transcribing") notifyStarted();
    }).then((text) => {
      const chunk = text.trim();
      if (this.disposed || this.segmentId !== segmentId || !chunk) return;
      this.transcript = joinTranscript(this.transcript, chunk);
      this.onTranscript(this.transcript);
    }).catch((cause) => {
      const message = cause instanceof Error ? cause.message : String(cause);
      if (!this.disposed && this.segmentId === segmentId && !isExpectedEmptyChunk(message)) {
        this.onError(message);
      }
    }).finally(() => {
      notifyStarted();
      samples.fill(0);
      if (this.active?.runId !== runId) return;
      this.active = null;
      this.pump();
    });
    this.active = { runId, started, promise };
  }

  async cancelCurrent(): Promise<void> {
    const active = this.active;
    if (!active) return;
    await Promise.race([active.started, active.promise]);
    await this.requestCancel(active.runId).catch(() => undefined);
  }

  dispose(): void {
    this.disposed = true;
    this.advanceSegment();
    void this.cancelCurrent();
  }
}

function isExpectedEmptyChunk(message: string): boolean {
  return message.startsWith("ASR_NO_SPEECH")
    || message.startsWith("TARGET_SPEAKER_REJECTED")
    || message.includes("completed without a transcript")
    || message.includes("cancelled");
}

function joinTranscript(current: string, next: string): string {
  if (!current) return next;
  const left = current[current.length - 1] ?? "";
  const right = next[0] ?? "";
  const separator = /[A-Za-z0-9]$/.test(left) && /^[A-Za-z0-9]/.test(right) ? " " : "";
  return `${current}${separator}${next}`;
}
