export const DEFAULT_CONVERSATION_TIMEOUT_MS = 1_800_000;
export const MIN_CONVERSATION_TIMEOUT_MS = 1_000;
export const MAX_CONVERSATION_TIMEOUT_MS = 3_600_000;

export const MIN_CONVERSATION_TIMEOUT_SECONDS = MIN_CONVERSATION_TIMEOUT_MS / 1_000;
export const MAX_CONVERSATION_TIMEOUT_SECONDS = MAX_CONVERSATION_TIMEOUT_MS / 1_000;

export function conversationTimeoutSecondsInputValue(timeoutMs: number): string {
  return String(timeoutMs / 1_000);
}

export function conversationTimeoutMsFromSecondsInput(value: string): number | null {
  if (!value || value.trim() !== value) return null;
  const timeoutMs = Number(value) * 1_000;
  return Number.isInteger(timeoutMs)
    && timeoutMs >= MIN_CONVERSATION_TIMEOUT_MS
    && timeoutMs <= MAX_CONVERSATION_TIMEOUT_MS
    ? timeoutMs
    : null;
}
