export const STREAM_CHUNK_SIZE = 4_096;

export type StreamingTextProjection = {
  chunks: readonly string[];
  tail: string;
  length: number;
};

export const EMPTY_STREAMING_TEXT: StreamingTextProjection = {
  chunks: [],
  tail: "",
  length: 0,
};

export class StreamingTextBuffer {
  private readonly chunks: string[] = [];
  private tail = "";
  private length = 0;

  append(delta: string): void {
    this.length += delta.length;
    this.tail += delta;
    while (this.tail.length >= STREAM_CHUNK_SIZE) {
      let splitAt = STREAM_CHUNK_SIZE;
      const previous = this.tail.charCodeAt(splitAt - 1);
      const next = this.tail.charCodeAt(splitAt);
      if (previous >= 0xd800 && previous <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) {
        splitAt -= 1;
      }
      this.chunks.push(this.tail.slice(0, splitAt));
      this.tail = this.tail.slice(splitAt);
    }
  }

  snapshot(): StreamingTextProjection {
    return {
      chunks: this.chunks.slice(),
      tail: this.tail,
      length: this.length,
    };
  }

  isEmpty(): boolean {
    return this.length === 0;
  }
}
