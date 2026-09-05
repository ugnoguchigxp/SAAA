import { AUDIO_PACKET_BYTES } from "./voiceAsrPacketizer";
import type { CommitReason } from "../../lib/generated/voiceAsr";
type Control = { resolve: () => void; reject: (error: Error) => void };
type Operation = { type: "audio"; bytes: Uint8Array } | ({ type: "commit"; reason: CommitReason } & Control) | ({ type: "stop"; finalizeCurrent: boolean } & Control);
type SenderApi = { append: (sequence: number, bytes: Uint8Array) => Promise<void>; commit: (reason: CommitReason) => Promise<void>; stop: (finalizeCurrent: boolean) => Promise<void> };

/** A single serial queue preserves audio → commit → audio ordering. */
export class VoiceAsrPacketSender {
  private readonly operations: Operation[] = [];
  private draining = false;
  private sequence = 0;
  private commitControl: Promise<void> | null = null;
  private stopControl: Promise<void> | null = null;
  private failure: Error | null = null;
  private pendingAudio = 0;
  private activeAudio: Uint8Array | null = null;
  private acceptingAudio = true;
  constructor(private readonly api: SenderApi, private readonly onFailure: (error: Error) => void = () => {}) {}

  enqueueAudio(bytes: Uint8Array): void {
    if (bytes.byteLength !== AUDIO_PACKET_BYTES) {
      bytes.fill(0);
      throw new Error("ASR packet has an invalid byte length.");
    }
    if (this.failure) {
      bytes.fill(0);
      throw this.failure;
    }
    if (!this.acceptingAudio) {
      bytes.fill(0);
      throw new Error("ASR_SESSION_STOPPING");
    }
    if (this.pendingAudio >= 10) {
      const error = new Error("ASR_BACKPRESSURE");
      bytes.fill(0);
      this.fail(error);
      throw error;
    }
    this.pendingAudio += 1;
    this.operations.push({ type: "audio", bytes });
    void this.drain();
  }
  enqueueCommit(reason: CommitReason): Promise<void> {
    if (this.commitControl) return this.commitControl;
    return this.enqueueControl("commit", reason);
  }
  enqueueStop(finalizeCurrent: boolean): Promise<void> {
    this.acceptingAudio = false;
    if (this.stopControl) return this.stopControl;
    return this.enqueueControl("stop", finalizeCurrent);
  }
  private enqueueControl(type: "commit", value: CommitReason): Promise<void>;
  private enqueueControl(type: "stop", value: boolean): Promise<void>;
  private enqueueControl(type: "commit" | "stop", value: CommitReason | boolean): Promise<void> {
    if (this.failure) return Promise.reject(this.failure);
    let resolveControl!: () => void;
    let rejectControl!: (error: Error) => void;
    const completion = new Promise<void>((resolve, reject) => { resolveControl = resolve; rejectControl = reject; });
    if (type === "commit") {
      this.commitControl = completion;
      this.operations.push({ type, reason: value as CommitReason, resolve: resolveControl, reject: rejectControl });
    } else {
      this.stopControl = completion;
      this.operations.push({ type, finalizeCurrent: value as boolean, resolve: resolveControl, reject: rejectControl });
    }
    void this.drain();
    return completion;
  }
  private async drain(): Promise<void> {
    if (this.draining) return;
    this.draining = true;
    let active: Operation | null = null;
    try { while (this.operations.length) {
      const operation = this.operations.shift()!;
      active = operation;
      if (operation.type === "audio") {
        this.activeAudio = operation.bytes;
        await this.api.append(this.sequence, operation.bytes);
        operation.bytes.fill(0);
        this.activeAudio = null;
        if (this.failure) throw this.failure;
        this.sequence += 1;
        this.pendingAudio -= 1;
      }
      else if (operation.type === "commit") {
        // The backend resolves commit as soon as its bounded actor queue accepts
        // the utterance. This is the ordering barrier; final recognition remains asynchronous.
        await this.api.commit(operation.reason);
        operation.resolve();
        this.commitControl = null;
      }
      else { await this.api.stop(operation.finalizeCurrent); operation.resolve(); this.stopControl = null; }
      active = null;
    } } catch (cause) {
      const error = cause instanceof Error ? cause : new Error(String(cause));
      if (active?.type === "audio") active.bytes.fill(0);
      else active?.reject(error);
      this.fail(error);
    } finally { this.draining = false; }
  }
  private fail(error: Error): void {
    if (this.failure) return;
    this.failure = error;
    this.activeAudio?.fill(0);
    this.activeAudio = null;
    this.operations.splice(0).forEach((operation) => {
      if (operation.type === "audio") operation.bytes.fill(0);
      else operation.reject(error);
    });
    this.commitControl = null;
    this.stopControl = null;
    this.pendingAudio = 0;
    this.onFailure(error);
  }
}
