import type { RuntimeEvent } from "../../lib/contracts";

export type SpeechRetry = { kind: "speech"; text: string; conversationId: string };

export class FinalResponseSpeechGate {
  private readonly completedMessageIds = new Set<string>();

  accept(event: RuntimeEvent): string | null {
    if (event.type !== "messageCompleted" || this.completedMessageIds.has(event.message.id)) return null;
    this.completedMessageIds.add(event.message.id);
    if (this.completedMessageIds.size > 64) {
      const oldest = this.completedMessageIds.values().next().value;
      if (oldest) this.completedMessageIds.delete(oldest);
    }
    return event.message.content;
  }
}

export function speechRetry(text: string, conversationId: string): SpeechRetry {
  return { kind: "speech", text, conversationId };
}
