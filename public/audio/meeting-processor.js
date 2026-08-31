class MeetingProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.frameLength = Math.max(1, Math.round(sampleRate / 10));
    this.buffer = new Float32Array(this.frameLength);
    this.offset = 0;
    this.port.onmessage = (event) => {
      if (event.data?.type !== "flush") return;
      if (this.offset > 0) {
        const completed = this.buffer.slice(0, this.offset);
        this.port.postMessage(completed, [completed.buffer]);
        this.buffer = new Float32Array(this.frameLength);
        this.offset = 0;
      }
      this.port.postMessage({ type: "flushed" });
    };
  }

  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input) return true;
    let sourceOffset = 0;
    while (sourceOffset < input.length) {
      const length = Math.min(input.length - sourceOffset, this.buffer.length - this.offset);
      this.buffer.set(input.subarray(sourceOffset, sourceOffset + length), this.offset);
      this.offset += length;
      sourceOffset += length;
      if (this.offset === this.buffer.length) {
        const completed = this.buffer;
        this.port.postMessage(completed, [completed.buffer]);
        this.buffer = new Float32Array(this.frameLength);
        this.offset = 0;
      }
    }
    return true;
  }
}
registerProcessor("meeting-processor", MeetingProcessor);
