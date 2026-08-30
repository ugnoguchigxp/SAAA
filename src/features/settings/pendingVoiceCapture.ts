export type PendingVoiceCapture = {
  stream: MediaStream | null;
  context: AudioContext | null;
  releaseLease: (() => void) | null;
};

export async function disposePendingVoiceCapture(pending: PendingVoiceCapture): Promise<void> {
  const stream = pending.stream;
  pending.stream = null;
  stream?.getTracks().forEach((track) => track.stop());
  const context = pending.context;
  pending.context = null;
  if (context) await context.close().catch(() => undefined);
  const releaseLease = pending.releaseLease;
  pending.releaseLease = null;
  releaseLease?.();
}
