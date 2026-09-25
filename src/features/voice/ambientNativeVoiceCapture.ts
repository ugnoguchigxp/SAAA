import type { MutableRefObject } from "react";
import type { VoiceSettings } from "../../lib/contracts";
import {
  audioBackendStatus,
  nativeCapturePreferred,
  startNativeVoiceCapture,
  stopNativeVoiceCapture,
} from "../../lib/audioBackend";
import type { VoiceActivityDetector } from "../../lib/voiceActivity";
import type { VoiceSessionEvent } from "../../lib/voiceSession";

export async function tryStartNativeVoiceCapture(input: {
  settings: VoiceSettings;
  nativeCapture?: MutableRefObject<boolean>;
  nativeStop?: MutableRefObject<(() => Promise<void>) | null>;
  activityDetector: MutableRefObject<VoiceActivityDetector | null>;
  createDetector: (sampleRate: number) => VoiceActivityDetector;
  stale: () => boolean;
  handleFrame: (frame: Float32Array) => void;
  disposeOwnedCapture: () => Promise<void>;
  applyEvent: (event: VoiceSessionEvent) => unknown;
  clearTranscript: () => void;
  onEnded?: (reason: string) => void;
}): Promise<boolean> {
  const native = await audioBackendStatus().catch(() => null);
  if (!native || !nativeCapturePreferred(native, input.settings.aecEnabled)) return false;
  try {
    const activityDetector = input.createDetector(16_000);
    input.activityDetector.current = activityDetector;
    if (input.nativeCapture) input.nativeCapture.current = true;
    await startNativeVoiceCapture(
      (frame) => {
        if (input.stale() || !input.nativeCapture?.current) return;
        input.handleFrame(frame);
      },
      (reason) => input.onEnded?.(reason),
    );
    if (input.stale()) {
      await stopNativeVoiceCapture().catch(() => undefined);
      if (input.nativeCapture) input.nativeCapture.current = false;
      await input.disposeOwnedCapture();
      return true;
    }
    if (input.nativeStop) input.nativeStop.current = () => stopNativeVoiceCapture();
    input.clearTranscript();
    input.applyEvent({ type: "captureStarted" });
    return true;
  } catch {
    if (input.nativeCapture) input.nativeCapture.current = false;
    await stopNativeVoiceCapture().catch(() => undefined);
    return false;
  }
}
