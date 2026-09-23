/**
 * Presentation-safe summaries of a conversation run.  Keep runtime/provider
 * diagnostics out of this type: they belong in logs, not in the UI.
 */
export type ConversationRuntimeActivity =
  | { type: "providerStarted"; providerId: string }
  | { type: "providerWorking" }
  | { type: "providerFailed" }
  | { type: "generationCancelled" }
  | { type: "voiceQueryQueued" }
  | { type: "webSearching" }
  | { type: "sourceFetching" }
  | { type: "sourceAvailable"; runId: string; url: string; title: string }
  | { type: "answerPreparing" };

export function appendConversationActivity(
  current: ConversationRuntimeActivity[],
  next: ConversationRuntimeActivity,
): ConversationRuntimeActivity[] {
  return [...current, next].slice(-8);
}

export function conversationActivityOutcome(
  activities: ConversationRuntimeActivity[],
): "cancelled-after-search" | "cancelled" | "failed" | null {
  const terminal = activities[activities.length - 1]?.type;
  if (terminal === "providerFailed") return "failed";
  if (terminal !== "generationCancelled") return null;
  return activities.some((activity) => activity.type === "answerPreparing")
    ? "cancelled-after-search"
    : "cancelled";
}
