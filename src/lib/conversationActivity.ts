/**
 * Presentation-safe summaries of a conversation run.  Keep runtime/provider
 * diagnostics out of this type: they belong in logs, not in the UI.
 */
export type ConversationRuntimeActivity =
  | { type: "providerStarted"; providerId: string }
  | { type: "providerSelected"; providerId: string; fallbackUsed: boolean }
  | { type: "providerWorking" }
  | { type: "providerFailed" }
  | { type: "generationCancelled" }
  | { type: "voiceQueryQueued" };

export function appendConversationActivity(
  current: ConversationRuntimeActivity[],
  next: ConversationRuntimeActivity,
): ConversationRuntimeActivity[] {
  return [...current, next].slice(-8);
}
