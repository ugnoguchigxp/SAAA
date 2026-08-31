export type FinalVoiceUtterance = {
  sessionId: string;
  utteranceId: string;
  conversationId: string;
  text: string;
};

type Entry = { value: FinalVoiceUtterance; claimed: boolean };

/** Keeps at most two finals until downstream turn delivery is confirmed. */
export class VoiceFinalDeliveryQueue {
  private values: Entry[] = [];
  private seen = new Set<string>();

  push(value: FinalVoiceUtterance): "accepted" | "duplicate" | "full" {
    if (this.seen.has(value.utteranceId)) return "duplicate";
    if (this.values.length >= 2) return "full";
    this.seen.add(value.utteranceId);
    this.values.push({ value, claimed: false });
    return "accepted";
  }

  claim(utteranceId: string): FinalVoiceUtterance | undefined {
    const entry = this.values.find((candidate) => candidate.value.utteranceId === utteranceId);
    if (!entry || entry.claimed) return undefined;
    entry.claimed = true;
    return entry.value;
  }

  settle(utteranceId: string, delivered: boolean): void {
    const index = this.values.findIndex((entry) => entry.value.utteranceId === utteranceId);
    if (index < 0) return;
    if (delivered) this.values.splice(index, 1);
    else this.values[index].claimed = false;
  }

  clear(): void {
    this.values = [];
    this.seen.clear();
  }
}
