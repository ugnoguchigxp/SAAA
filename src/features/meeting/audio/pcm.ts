export function mixChannels(buffer: AudioBuffer): Float32Array {
  const result = new Float32Array(buffer.length);
  for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
    const samples = buffer.getChannelData(channel);
    for (let index = 0; index < samples.length; index += 1) result[index] += samples[index] / buffer.numberOfChannels;
  }
  return result;
}

export function appendFrames(target: Float32Array, frames: Float32Array[]): Float32Array {
  const length = frames.reduce((total, frame) => total + frame.length, target.length);
  const merged = new Float32Array(length); merged.set(target); let offset = target.length;
  frames.forEach((frame) => { merged.set(frame, offset); offset += frame.length; }); return merged;
}
