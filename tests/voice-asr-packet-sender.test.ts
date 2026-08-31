import { describe, expect, test } from "bun:test";
import { VoiceAsrPacketSender } from "../src/features/voice/voiceAsrPacketSender";

const packet = () => new Uint8Array(3_200);
describe("VoiceAsrPacketSender", () => {
  test("preserves audio → commit → audio order and only advances successful sequences", async () => {
    const log: string[] = [];
    const sender = new VoiceAsrPacketSender({ append: async (sequence) => { log.push(`audio:${sequence}`); }, commit: async () => { log.push("commit"); }, stop: async () => { log.push("stop"); } });
    const first = packet().fill(7);
    sender.enqueueAudio(first);
    const commit = sender.enqueueCommit("silence");
    sender.enqueueAudio(packet());
    await commit;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["audio:0", "commit", "audio:1"]);
    expect(first.every((byte) => byte === 0)).toBeTrue();
  });
  test("queues stop after a pending commit and waits for its own completion", async () => {
    const log: string[] = [];
    let releaseAppend!: () => void;
    const appendStarted = new Promise<void>((resolve) => { releaseAppend = resolve; });
    const sender = new VoiceAsrPacketSender({
      append: async () => { log.push("audio"); await appendStarted; },
      commit: async () => { log.push("commit"); },
      stop: async () => { log.push("stop"); },
    });
    sender.enqueueAudio(packet());
    const commit = sender.enqueueCommit("silence");
    const stop = sender.enqueueStop(true);
    let stopCompleted = false;
    void stop.then(() => { stopCompleted = true; });
    await Promise.resolve();
    expect(stopCompleted).toBeFalse();
    releaseAppend();
    await Promise.all([commit, stop]);
    expect(log).toEqual(["audio", "commit", "stop"]);
  });
  test("rejects and zeroizes microphone frames that arrive after stop is queued", async () => {
    let releaseStop!: () => void;
    const stopPending = new Promise<void>((resolve) => { releaseStop = resolve; });
    const sender = new VoiceAsrPacketSender({
      append: async () => {},
      commit: async () => {},
      stop: async () => stopPending,
    });
    const stop = sender.enqueueStop(true);
    const late = packet().fill(6);
    expect(() => sender.enqueueAudio(late)).toThrow("ASR_SESSION_STOPPING");
    expect(late.every((byte) => byte === 0)).toBeTrue();
    releaseStop();
    await stop;
  });
  test("uses backend commit acceptance as the next-packet ordering barrier", async () => {
    const log: string[] = [];
    let releaseCommit!: () => void;
    const commitFinished = new Promise<void>((resolve) => { releaseCommit = resolve; });
    const sender = new VoiceAsrPacketSender({
      append: async (sequence) => { log.push(`audio:${sequence}`); },
      commit: async () => { log.push("commit"); await commitFinished; },
      stop: async () => { log.push("stop"); },
    });
    sender.enqueueAudio(packet());
    const commit = sender.enqueueCommit("silence");
    sender.enqueueAudio(packet());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["audio:0", "commit"]);
    releaseCommit();
    await commit;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["audio:0", "commit", "audio:1"]);
  });
  test("fails closed at ten unsent packets and zeroizes queued audio", async () => {
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => { release = resolve; });
    const queued = Array.from({ length: 11 }, () => packet().fill(7));
    const sender = new VoiceAsrPacketSender({
      append: async () => blocked,
      commit: async () => {},
      stop: async () => {},
    });
    for (const value of queued.slice(0, 10)) sender.enqueueAudio(value);
    expect(() => sender.enqueueAudio(queued[10])).toThrow("ASR_BACKPRESSURE");
    expect(queued.slice(0, 10).every((value) => value.every((byte) => byte === 0))).toBeTrue();
    release();
  });
  test("propagates invoke failure and never advances the sequence", async () => {
    const sequences: number[] = [];
    const sender = new VoiceAsrPacketSender({
      append: async (sequence) => { sequences.push(sequence); throw new Error("invoke failed"); },
      commit: async () => {},
      stop: async () => {},
    });
    const first = packet().fill(9);
    sender.enqueueAudio(first);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(sequences).toEqual([0]);
    expect(first.every((byte) => byte === 0)).toBeTrue();
    expect(() => sender.enqueueAudio(packet())).toThrow("invoke failed");
  });
});
