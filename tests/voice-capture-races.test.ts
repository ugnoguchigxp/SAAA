import { expect, test } from "bun:test";
import { VoiceCaptureResources } from "../src/features/voice/VoiceCaptureResources";
import type { VoiceAsrPacketSender } from "../src/features/voice/voiceAsrPacketSender";
import { installJsdom } from "./jsdomGlobals";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("overlapping detaches and disposal cannot touch replacement capture resources", async () => {
  const owner = new VoiceCaptureResources();
  const close = deferred();
  let closes = 0;
  const stops: string[] = [];
  const sender = (id: string) =>
    ({
      enqueueStop: async () => {
        stops.push(id);
      },
    }) as unknown as VoiceAsrPacketSender;
  owner.voiceContextRef.current = {
    close: () => {
      closes++;
      return close.promise;
    },
  } as AudioContext;
  owner.voiceAsrSenderRef.current = sender("old");
  const first = owner.detachVoiceCapture(false);
  const second = owner.detachVoiceCapture(false);
  owner.dispose();
  const replacement = {
    close: async () => {
      closes++;
    },
  } as AudioContext;
  const replacementSender = sender("new");
  owner.voiceContextRef.current = replacement;
  owner.voiceAsrSenderRef.current = replacementSender;
  close.resolve();
  await Promise.all([first, second]);
  expect(closes).toBe(1);
  expect(stops).toEqual(["old"]);
  expect(owner.voiceContextRef.current).toBe(replacement);
  expect(owner.voiceAsrSenderRef.current).toBe(replacementSender);
});

test("dispose during flush invalidates the old detach before replacement attachment", async () => {
  const env = installJsdom();
  const owner = new VoiceCaptureResources();
  let disconnects = 0;
  owner.voiceNodeRef.current = {
    port: { postMessage: () => {}, onmessage: null },
    disconnect: () => {
      disconnects++;
    },
  } as unknown as AudioWorkletNode;
  try {
    const detached = owner.detachVoiceCapture(true);
    owner.dispose();
    const replacement = { close: async () => {} } as AudioContext;
    owner.voiceContextRef.current = replacement;
    await detached;
    expect(disconnects).toBe(1);
    expect(owner.voiceContextRef.current).toBe(replacement);
    expect(owner.voiceFlushResolverRef.current).toBeNull();
  } finally {
    env.dom.window.close();
    env.restore();
  }
});
