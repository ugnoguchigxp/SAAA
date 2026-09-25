import { describe, expect, test } from "bun:test";
import { nativeCapturePreferred, type AudioBackendStatus } from "../src/lib/audioBackend";

const unavailable: AudioBackendStatus = {
  available: false,
  reason: "unsupported",
  captureActive: false,
  playbackActive: false,
  aecActive: false,
  duckingLevel: "min",
  agcEnabled: false,
  outputTransport: null,
  macosMajor: 13,
};

const available: AudioBackendStatus = {
  ...unavailable,
  available: true,
  reason: null,
  macosMajor: 26,
};

describe("nativeCapturePreferred", () => {
  test("uses VoiceProcessing only when the backend is available and AEC is on", () => {
    expect(nativeCapturePreferred(available, true)).toBe(true);
    expect(nativeCapturePreferred(available, false)).toBe(false);
    expect(nativeCapturePreferred(unavailable, true)).toBe(false);
  });
});
