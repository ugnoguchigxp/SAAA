export type StreamingSpeechChunkerOptions = {
  minSentenceChars?: number;
  targetChars?: number;
  maxChars?: number;
};

const SENTENCE_END = new Set(["。", "！", "？", "!", "?", "\n"]);
const CLOSING_MARKS = new Set(["」", "』", "）", ")", "］", "]", "】", "”", "’", "\"", "'"]);
const SOFT_BREAK = new Set(["、", "，", ",", "；", ";", "：", ":"]);

export class StreamingSpeechChunker {
  private buffer = "";
  private readonly minSentenceChars: number;
  private readonly targetChars: number;
  private readonly maxChars: number;

  constructor({
    minSentenceChars = 12,
    targetChars = 36,
    maxChars = 64,
  }: StreamingSpeechChunkerOptions = {}) {
    if (!(minSentenceChars > 0 && minSentenceChars <= targetChars && targetChars <= maxChars)) {
      throw new RangeError("Speech chunk sizes must satisfy 0 < min <= target <= max");
    }
    this.minSentenceChars = minSentenceChars;
    this.targetChars = targetChars;
    this.maxChars = maxChars;
  }

  push(text: string): string[] {
    if (text) this.buffer += text;
    return this.drain(false);
  }

  finish(): string[] {
    return this.drain(true);
  }

  reset(): void {
    this.buffer = "";
  }

  private drain(final: boolean): string[] {
    const chunks: string[] = [];
    while (true) {
      this.buffer = this.buffer.trimStart();
      const characters = Array.from(this.buffer);
      if (characters.length === 0) break;

      const sentenceBoundary = findSentenceBoundary(
        characters,
        this.minSentenceChars,
        this.maxChars,
      );
      if (sentenceBoundary !== null) {
        this.take(characters, sentenceBoundary, chunks);
        continue;
      }

      if (characters.length >= this.targetChars) {
        const searchLimit = characters.length >= this.maxChars
          ? this.maxChars
          : this.targetChars;
        const softBoundary = findSoftBoundary(
          characters,
          this.minSentenceChars,
          searchLimit,
        );
        if (softBoundary !== null) {
          this.take(characters, softBoundary, chunks);
          continue;
        }
      }

      if (characters.length >= this.maxChars) {
        this.take(characters, this.maxChars, chunks);
        continue;
      }

      if (final) this.take(characters, characters.length, chunks);
      break;
    }
    return chunks;
  }

  private take(characters: string[], count: number, chunks: string[]): void {
    const chunk = characters.slice(0, count).join("").trim();
    this.buffer = characters.slice(count).join("");
    if (chunk) chunks.push(chunk);
  }
}

function findSentenceBoundary(
  characters: string[],
  minimum: number,
  maximum: number,
): number | null {
  const limit = Math.min(characters.length, maximum);
  for (let index = 0; index < limit; index += 1) {
    if (!SENTENCE_END.has(characters[index])) continue;
    if (isUrlQuestionMark(characters, index)) continue;
    let boundary = index + 1;
    while (boundary < limit && CLOSING_MARKS.has(characters[boundary])) boundary += 1;
    if (boundary >= minimum) return boundary;
  }
  return null;
}

function findSoftBoundary(
  characters: string[],
  minimum: number,
  limit: number,
): number | null {
  for (let index = Math.min(limit, characters.length) - 1; index >= minimum - 1; index -= 1) {
    const character = characters[index];
    if (SOFT_BREAK.has(character)) return index + 1;
    if (/\s/u.test(character)) return index;
  }
  return null;
}

function isUrlQuestionMark(characters: string[], index: number): boolean {
  if (characters[index] !== "?") return false;
  let tokenStart = index;
  while (tokenStart > 0 && !/\s/u.test(characters[tokenStart - 1])) tokenStart -= 1;
  const prefix = characters.slice(tokenStart, index).join("").toLowerCase();
  return prefix.includes("http://") || prefix.includes("https://") || prefix.includes("www.");
}
