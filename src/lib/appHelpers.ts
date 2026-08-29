import type { MeetingState } from "./contracts";

export function toMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function isMeetingBlocking(state: MeetingState): boolean {
  return state === "preflight" || state === "active" || state === "paused" || state === "stopping";
}
