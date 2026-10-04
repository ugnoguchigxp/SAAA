import { state, updateEntry } from "./conversationAsrState";
import type { createConversationCaptureAudit } from "./conversationCaptureAudit";

export function queueRecognizedDelivery(
  id: string,
  auditCapture: ReturnType<typeof createConversationCaptureAudit>,
): boolean {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (!entry || entry.status !== "completed" || !entry.text?.trim() || entry.deliveryQueued)
    return false;
  updateEntry(id, { deliveryQueued: true });
  auditCapture("conversation-asr-delivery-queued", "decision", id, {
    textBytes: entry.text.length,
  });
  return true;
}

export function failConversationAsrDelivery(id: string, error: string) {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (entry?.deliveryQueued && entry.provider)
    updateEntry(id, { status: "failed", deliveryQueued: false, error });
}

export function retryConversationAsrDelivery(id: string): string | null {
  const entry = state.entries.find((candidate) => candidate.id === id);
  if (
    !entry ||
    entry.status !== "failed" ||
    !entry.provider ||
    !entry.text?.trim() ||
    entry.deliveryQueued
  )
    return null;
  updateEntry(id, { status: "completed", deliveryQueued: true, error: null });
  return entry.text;
}
