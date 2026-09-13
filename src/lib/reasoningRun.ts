// Transport identity only; no model name or credentials cross into the UI.
const runs = new Map<string, string>();
const cancelled = new Set<string>();
export function markReasoningRun(runId: string, conversationId: string): void {
  runs.set(runId, conversationId);
}
export function endReasoningRun(runId: string): void {
  runs.delete(runId);
  cancelled.delete(runId);
}
export function isReasoningRun(runId: string | null): boolean {
  return runId !== null && runs.has(runId);
}

export function markReasoningCancellation(runId: string): void {
  if (runs.has(runId)) cancelled.add(runId);
}
export function reasoningCancellationRequested(runId: string): boolean {
  return cancelled.has(runId);
}

import type { PendingConversationPrompt, SubmitPromptOptions } from "./conversationSession";
export function queueReasoningReplacement(
  runId: string | null,
  prompt: string,
  pending: PendingConversationPrompt[],
  options: SubmitPromptOptions,
  conversationId: string,
): "queued" | "full" | null {
  if (
    !runId ||
    runs.get(runId) !== conversationId ||
    !isReasoningRun(runId) ||
    !prompt.trim() ||
    options.retryInputMessageId
  )
    return null;
  if (pending.length >= 2) {
    options.onSettled?.(false);
    return "full";
  }
  pending.push({
    content: prompt.trim(),
    inputOrigin: options.inputOrigin ?? "text",
    sourceId: options.sourceId ?? undefined,
    onSettled: options.onSettled,
  });
  return "queued";
}

export function clearReasoningCancellation(runId: string): void {
  cancelled.delete(runId);
}
