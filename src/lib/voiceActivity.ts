export const DEFAULT_VOICE_SILENCE_TIMEOUT_MS = 1_500;
const DEFAULT_SPEECH_THRESHOLD_RMS = 0.008;
const DEFAULT_REQUIRED_SPEECH_MS = 240;
const DEFAULT_CANDIDATE_RESET_MS = 200;

type VoiceActivityDetectorOptions = {
  sampleRate: number;
  speechThresholdRms?: number;
  requiredSpeechMs?: number;
  silenceTimeoutMs?: number;
  candidateResetMs?: number;
};

export type VoiceActivityObservation = {
  hasSpeech: boolean;
  shouldFinalize: boolean;
  rms: number;
};

export class VoiceActivityDetector {
  private readonly speechThresholdRms: number;
  private readonly requiredSpeechSamples: number;
  private readonly silenceTimeoutSamples: number;
  private readonly candidateResetSamples: number;
  private speechSamples = 0;
  private candidateSilenceSamples = 0;
  private silenceSamples = 0;
  private speechDetected = false;
  private finalized = false;

  constructor({
    sampleRate,
    speechThresholdRms = DEFAULT_SPEECH_THRESHOLD_RMS,
    requiredSpeechMs = DEFAULT_REQUIRED_SPEECH_MS,
    silenceTimeoutMs = DEFAULT_VOICE_SILENCE_TIMEOUT_MS,
    candidateResetMs = DEFAULT_CANDIDATE_RESET_MS,
  }: VoiceActivityDetectorOptions) {
    if (!Number.isFinite(sampleRate) || sampleRate <= 0) throw new RangeError("sampleRate must be positive");
    if (!Number.isFinite(speechThresholdRms) || speechThresholdRms <= 0) throw new RangeError("speechThresholdRms must be positive");
    this.speechThresholdRms = speechThresholdRms;
    this.requiredSpeechSamples = millisecondsToSamples(requiredSpeechMs, sampleRate);
    this.silenceTimeoutSamples = millisecondsToSamples(silenceTimeoutMs, sampleRate);
    this.candidateResetSamples = millisecondsToSamples(candidateResetMs, sampleRate);
  }

  observe(frame: Float32Array): VoiceActivityObservation {
    const rms = calculateRms(frame);
    if (this.finalized || frame.length === 0) {
      return { hasSpeech: this.speechDetected, shouldFinalize: false, rms };
    }

    const voiced = rms >= this.speechThresholdRms;
    if (!this.speechDetected) {
      if (voiced) {
        this.speechSamples += frame.length;
        this.candidateSilenceSamples = 0;
        if (this.speechSamples >= this.requiredSpeechSamples) {
          this.speechDetected = true;
          this.silenceSamples = 0;
        }
      } else if (this.speechSamples > 0) {
        this.candidateSilenceSamples += frame.length;
        if (this.candidateSilenceSamples >= this.candidateResetSamples) {
          this.speechSamples = 0;
          this.candidateSilenceSamples = 0;
        }
      }
      return { hasSpeech: this.speechDetected, shouldFinalize: false, rms };
    }

    if (voiced) {
      this.silenceSamples = 0;
      return { hasSpeech: true, shouldFinalize: false, rms };
    }

    this.silenceSamples += frame.length;
    if (this.silenceSamples < this.silenceTimeoutSamples) {
      return { hasSpeech: true, shouldFinalize: false, rms };
    }

    this.finalized = true;
    return { hasSpeech: true, shouldFinalize: true, rms };
  }
}

function millisecondsToSamples(milliseconds: number, sampleRate: number): number {
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) throw new RangeError("duration must be positive");
  return Math.ceil((milliseconds / 1_000) * sampleRate);
}

function calculateRms(frame: Float32Array): number {
  if (frame.length === 0) return 0;
  let sum = 0;
  for (const sample of frame) sum += sample;
  const mean = sum / frame.length;
  let sumOfSquares = 0;
  for (const sample of frame) {
    const centered = sample - mean;
    sumOfSquares += centered * centered;
  }
  return Math.sqrt(sumOfSquares / frame.length);
}
