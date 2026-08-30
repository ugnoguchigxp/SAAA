const FILTER_RADIUS = 24;
const CUTOFF_GUARD = 0.94;

/** Band-limited windowed-sinc resampling. The low-pass cutoff prevents aliasing on downsampling. */
export function resamplePcm(samples: Float32Array, sourceRate: number, targetRate: number): Float32Array {
  if (!samples.length || !validRate(sourceRate) || !validRate(targetRate)) return new Float32Array();
  if (sourceRate === targetRate) return samples.slice();
  const ratio = sourceRate / targetRate;
  const cutoff = Math.min(1, targetRate / sourceRate) * CUTOFF_GUARD;
  const output = new Float32Array(Math.floor(samples.length / ratio));
  for (let outputIndex = 0; outputIndex < output.length; outputIndex += 1) {
    const position = outputIndex * ratio;
    const center = Math.floor(position);
    let weighted = 0;
    let weightSum = 0;
    for (let sourceIndex = center - FILTER_RADIUS + 1; sourceIndex <= center + FILTER_RADIUS; sourceIndex += 1) {
      if (sourceIndex < 0 || sourceIndex >= samples.length) continue;
      const distance = position - sourceIndex;
      const windowPosition = Math.abs(distance) / FILTER_RADIUS;
      if (windowPosition >= 1) continue;
      const window = 0.5 * (1 + Math.cos(Math.PI * windowPosition));
      const scaled = Math.PI * cutoff * distance;
      const sinc = Math.abs(scaled) < 1e-8 ? cutoff : cutoff * Math.sin(scaled) / scaled;
      const weight = sinc * window;
      weighted += (samples[sourceIndex] ?? 0) * weight;
      weightSum += weight;
    }
    output[outputIndex] = weightSum === 0 ? 0 : weighted / weightSum;
  }
  return output;
}

function validRate(value: number): boolean {
  return Number.isFinite(value) && value >= 8_000 && value <= 192_000;
}
