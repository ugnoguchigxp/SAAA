import type { VoiceActivityDetector } from "../../lib/voiceActivity";
import { stopVoiceAsrSession } from "../../lib/voiceAsrRuntime";
import { VoiceAsrPacketizer } from "./voiceAsrPacketizer";
import { VoiceAsrPacketSender } from "./voiceAsrPacketSender";
import { initialVoiceAsrProjection } from "./voiceAsrProjection";
import { VoiceFinalDeliveryQueue } from "./voiceFinalDeliveryQueue";
type VoiceAsrStopWaiter = { promise: Promise<void>; resolve: () => void };

/** Single owner of capture handles, ASR transport and final-delivery resources. */
export class VoiceCaptureResources {
  readonly voiceStreamRef = { current: null as MediaStream | null };
  readonly voiceContextRef = { current: null as AudioContext | null };
  readonly voiceSourceRef = { current: null as MediaStreamAudioSourceNode | null };
  readonly voiceNodeRef = { current: null as AudioWorkletNode | null };
  readonly voiceFlushResolverRef = { current: null as (() => void) | null };
  readonly voiceActivityDetectorRef = { current: null as VoiceActivityDetector | null };
  readonly voiceCaptureLeaseRef = { current: null as (() => void) | null };
  readonly voiceCaptureAttemptRef = { current: 0 };
  readonly voiceAsrPacketizerRef = { current: new VoiceAsrPacketizer() };
  readonly voiceAsrSenderRef = { current: null as VoiceAsrPacketSender | null };
  readonly voiceAsrSessionIdRef = { current: null as string | null };
  readonly acceptedVoiceAsrSessionsRef = { current: new Set<string>() };
  readonly voiceAsrConversationsRef = { current: new Map<string, string>() };
  readonly voiceAsrStopWaitersRef = { current: new Map<string, VoiceAsrStopWaiter>() };
  readonly voiceAsrPacketCountRef = { current: 0 };
  readonly voiceAsrProjectionRef = { current: initialVoiceAsrProjection };
  readonly voiceFinalDeliveryRef = { current: new VoiceFinalDeliveryQueue() };
  readonly disposedRef = { current: false };
  readonly listeningEnabledRef = { current: false };

  detachVoiceCapture = async (flush: boolean, stopSession = true) => {
    const attempt = ++this.voiceCaptureAttemptRef.current;
    this.voiceFlushResolverRef.current?.();
    if (flush && this.voiceNodeRef.current) {
      await new Promise<void>((resolve) => {
        let completed = false;
        const finish = () => {
          if (completed) return;
          completed = true;
          this.voiceFlushResolverRef.current = null;
          window.clearTimeout(timeout);
          resolve();
        };
        const timeout = window.setTimeout(finish, 250);
        this.voiceFlushResolverRef.current = finish;
        this.voiceNodeRef.current?.port.postMessage({ type: "flush" });
      });
    }
    if (attempt !== this.voiceCaptureAttemptRef.current) return;
    await this.releaseCapture(stopSession);
  };

  private releaseCapture = async (stopSession = true) => {
    // Detach ownership before the first await; later generations use new handles.
    const context = this.voiceContextRef.current;
    const lease = this.voiceCaptureLeaseRef.current;
    const sender = this.voiceAsrSenderRef.current;
    const sessionId = this.voiceAsrSessionIdRef.current;
    this.voiceContextRef.current = null;
    this.voiceCaptureLeaseRef.current = null;
    this.voiceAsrSenderRef.current = null;
    this.voiceAsrSessionIdRef.current = null;
    if (this.voiceNodeRef.current) this.voiceNodeRef.current.port.onmessage = null;
    this.voiceNodeRef.current?.disconnect();
    this.voiceSourceRef.current?.disconnect();
    this.voiceStreamRef.current?.getTracks().forEach((track) => track.stop());
    this.voiceNodeRef.current = null;
    this.voiceSourceRef.current = null;
    this.voiceStreamRef.current = null;
    this.voiceActivityDetectorRef.current = null;
    this.voiceAsrPacketizerRef.current.reset();
    lease?.();
    if (context) await context.close().catch(() => undefined);
    if (sender && stopSession) {
      const stopped = await sender.enqueueStop(false).then(
        () => true,
        () => false,
      );
      if (!stopped && sessionId) {
        await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
      }
    } else if (sessionId && stopSession) {
      await stopVoiceAsrSession({ sessionId, finalizeCurrent: false }).catch(() => undefined);
    }
  };

  packetVoiceFrame = (frame: Float32Array) => {
    const sender = this.voiceAsrSenderRef.current;
    if (!sender || this.disposedRef.current || !this.listeningEnabledRef.current) return;
    const packets = this.voiceAsrPacketizerRef.current.append(frame);
    for (const [index, packet] of packets.entries()) {
      try {
        sender.enqueueAudio(packet);
        this.voiceAsrPacketCountRef.current += 1;
      } catch {
        for (const unsent of packets.slice(index)) unsent.fill(0);
        break;
      }
    }
  };

  waitForVoiceAsrStopped = (sessionId: string): Promise<void> => {
    const existing = this.voiceAsrStopWaitersRef.current.get(sessionId);
    if (existing) return existing.promise;
    let complete!: () => void;
    const promise = new Promise<void>((resolve) => {
      const timeout = window.setTimeout(() => complete(), 17_000);
      complete = () => {
        window.clearTimeout(timeout);
        this.voiceAsrStopWaitersRef.current.delete(sessionId);
        resolve();
      };
    });
    this.voiceAsrStopWaitersRef.current.set(sessionId, { promise, resolve: complete });
    return promise;
  };

  dispose = () => {
    this.voiceCaptureAttemptRef.current += 1;
    this.voiceFlushResolverRef.current?.();
    void this.releaseCapture();
    this.voiceFinalDeliveryRef.current.clear();
    this.voiceAsrStopWaitersRef.current.forEach((waiter) => waiter.resolve());
    this.voiceAsrStopWaitersRef.current.clear();
    this.acceptedVoiceAsrSessionsRef.current.clear();
    this.voiceAsrConversationsRef.current.clear();
  };
}
