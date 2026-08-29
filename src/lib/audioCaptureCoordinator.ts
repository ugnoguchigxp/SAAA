export type AudioCaptureOwner = "chat" | "meeting" | "voice-enrollment";

let activeLease: { owner: AudioCaptureOwner; token: symbol } | null = null;

export function acquireAudioCapture(owner: AudioCaptureOwner): () => void {
  if (activeLease) {
    throw new Error(`Microphone capture is already in use by ${activeLease.owner}.`);
  }
  const token = Symbol(owner);
  activeLease = { owner, token };
  return () => {
    if (activeLease?.token === token) activeLease = null;
  };
}

export function currentAudioCaptureOwner(): AudioCaptureOwner | null {
  return activeLease?.owner ?? null;
}
