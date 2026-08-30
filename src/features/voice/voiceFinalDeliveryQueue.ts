export type FinalVoiceUtterance = { sessionId: string; utteranceId: string; conversationId: string; text: string };
export class VoiceFinalDeliveryQueue {
  private values: FinalVoiceUtterance[] = []; private seen = new Set<string>();
  push(value: FinalVoiceUtterance): "accepted" | "duplicate" | "full" { if (this.seen.has(value.utteranceId)) return "duplicate"; if (this.values.length >= 2) return "full"; this.seen.add(value.utteranceId); this.values.push(value); return "accepted"; }
  shift(): FinalVoiceUtterance | undefined { return this.values.shift(); }
  clear(): void { this.values = []; this.seen.clear(); }
}
