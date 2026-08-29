export function mergePcmFrames(frames: Float32Array[], length: number): Float32Array {
  const merged = new Float32Array(length);
  let offset = 0;
  for (const frame of frames) {
    merged.set(frame, offset);
    offset += frame.length;
  }
  return merged;
}
