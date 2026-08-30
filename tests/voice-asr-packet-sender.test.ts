import { describe, expect, test } from "bun:test";
import { VoiceAsrPacketSender } from "../src/features/voice/voiceAsrPacketSender";

const packet = () => new Uint8Array(3_200);
describe("VoiceAsrPacketSender", () => {
  test("preserves audio → commit → audio order and only advances successful sequences", async () => {
    const log: string[] = [];
    const sender = new VoiceAsrPacketSender({ append: async (sequence) => { log.push(`audio:${sequence}`); }, commit: async () => { log.push("commit"); }, stop: async () => { log.push("stop"); } });
    sender.enqueueAudio(packet());
    const commit = sender.enqueueCommit("silence");
    sender.enqueueAudio(packet());
    await commit;
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["audio:0", "commit", "audio:1"]);
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
  test("does not pause later microphone packets for final recognition", async () => {
    const log: string[] = [];
    let releaseCommit!: () => void;
    const commitFinished = new Promise<void>((resolve) => { releaseCommit = resolve; });
    const sender = new VoiceAsrPacketSender({
      append: async (sequence) => { log.push(`audio:${sequence}`); },
      commit: async () => { log.push("commit"); await commitFinished; },
      stop: async () => { log.push("stop"); },
    });
    sender.enqueueAudio(packet());
    await sender.enqueueCommit("silence");
    sender.enqueueAudio(packet());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual(["audio:0", "commit", "audio:1"]);
    releaseCommit();
  });
});
