import { expect, test } from "bun:test";
import { VoiceCaptureResources } from "../src/features/voice/VoiceCaptureResources";
import { installJsdom } from "./jsdomGlobals";

test("capture disposal is idempotent and resolves ASR stop waiters", async () => {
  const env = installJsdom();
  const owner = new VoiceCaptureResources();
  const calls = { node: 0, source: 0, track: 0, context: 0, lease: 0 };
  owner.voiceNodeRef.current = {
    port: { onmessage: () => {} },
    disconnect: () => {
      calls.node += 1;
    },
  } as unknown as AudioWorkletNode;
  owner.voiceSourceRef.current = {
    disconnect: () => {
      calls.source += 1;
    },
  } as MediaStreamAudioSourceNode;
  owner.voiceStreamRef.current = {
    getTracks: () => [
      {
        stop: () => {
          calls.track += 1;
        },
      },
    ],
  } as unknown as MediaStream;
  owner.voiceContextRef.current = {
    close: async () => {
      calls.context += 1;
    },
  } as AudioContext;
  owner.voiceCaptureLeaseRef.current = () => {
    calls.lease += 1;
  };
  owner.acceptedVoiceAsrSessionsRef.current.add("session-fixture");
  const stopped = owner.waitForVoiceAsrStopped("session-fixture");
  try {
    owner.dispose();
    owner.dispose();
    await stopped;
    expect(calls).toEqual({ node: 1, source: 1, track: 1, context: 1, lease: 1 });
    expect(owner.voiceStreamRef.current).toBeNull();
    expect(owner.voiceAsrStopWaitersRef.current.size).toBe(0);
    expect(owner.acceptedVoiceAsrSessionsRef.current.size).toBe(0);
  } finally {
    env.dom.window.close();
    env.restore();
  }
});
