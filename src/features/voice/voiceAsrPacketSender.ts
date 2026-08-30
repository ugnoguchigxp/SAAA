import { AUDIO_PACKET_BYTES } from "./voiceAsrPacketizer";

export type CommitReason = "silence" | "max-duration";
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
  constructor(private readonly api: SenderApi, private readonly onFailure: (error: Error) => void = () => {}) {}

  enqueueAudio(bytes: Uint8Array): void {
    if (bytes.byteLength !== AUDIO_PACKET_BYTES) throw new Error("ASR packet has an invalid byte length.");
    if (this.failure) throw this.failure;
    if (this.operations.filter((operation) => operation.type === "audio").length >= 10) {
      const error = new Error("ASR_BACKPRESSURE");
      this.fail(error);
      throw error;
    }
    this.operations.push({ type: "audio", bytes });
    void this.drain();
  }
  enqueueCommit(reason: CommitReason): Promise<void> {
    if (this.commitControl) return this.commitControl;
    return this.enqueueControl("commit", reason);
  }
  enqueueStop(finalizeCurrent: boolean): Promise<void> {
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
      if (operation.type === "audio") { await this.api.append(this.sequence, operation.bytes); this.sequence += 1; }
      else if (operation.type === "commit") {
        // The backend owns final recognition after it accepts this command.
        // Waiting for that network-bound work would block fresh microphone
        // packets and defeat continuous capture, so only serialize dispatch.
        void this.api.commit(operation.reason).catch((cause) => this.fail(cause instanceof Error ? cause : new Error(String(cause))));
        operation.resolve();
        this.commitControl = null;
      }
      else { await this.api.stop(operation.finalizeCurrent); operation.resolve(); this.stopControl = null; }
      active = null;
    } } catch (cause) {
      const error = cause instanceof Error ? cause : new Error(String(cause));
      if (active?.type !== "audio") active?.reject(error);
      this.fail(error);
    } finally { this.draining = false; }
  }
  private fail(error: Error): void {
    if (this.failure) return;
    this.failure = error;
    this.operations.splice(0).forEach((operation) => {
      if (operation.type === "audio") operation.bytes.fill(0);
      else operation.reject(error);
    });
    this.commitControl = null;
    this.stopControl = null;
    this.onFailure(error);
  }
}
