import { describe, expect, test } from "bun:test";
import {
  ensureMicrophoneAudioContextRunning,
  enumerateAudioInputDevices,
  microphoneCaptureConstraints,
  MicrophoneCaptureError,
  requestMicrophoneStream,
  type MicrophoneEnvironment,
} from "../src/lib/microphone";

function environment(overrides: Partial<MicrophoneEnvironment> = {}): MicrophoneEnvironment {
  return {
    secureContext: true,
    mediaDevices: {
      getUserMedia: async () => ({} as MediaStream),
      enumerateDevices: async () => [],
    },
    ...overrides,
  };
}

describe("microphone capture", () => {
  test("enables echo cancellation for speech playback safety", () => {
    expect(microphoneCaptureConstraints()).toEqual({
      autoGainControl: true,
      echoCancellation: true,
      noiseSuppression: true,
    });
    expect(microphoneCaptureConstraints("microphone-a")).toEqual({
      autoGainControl: true,
      echoCancellation: true,
      noiseSuppression: true,
      deviceId: { exact: "microphone-a" },
    });
  });

  test("reports a build/API problem before requesting permission", async () => {
    const result = requestMicrophoneStream(true, environment({ mediaDevices: undefined }));
    await expect(result).rejects.toMatchObject({
      name: "MicrophoneCaptureError",
      code: "api-unavailable",
    });
  });

  test("distinguishes an insecure context from a missing app capability", async () => {
    const result = requestMicrophoneStream(true, environment({ mediaDevices: undefined, secureContext: false }));
    await expect(result).rejects.toThrow("secure application context");
  });

  test("classifies permission denial separately", async () => {
    const result = requestMicrophoneStream(true, environment({
      mediaDevices: {
        getUserMedia: async () => { throw new DOMException("Denied", "NotAllowedError"); },
        enumerateDevices: async () => [],
      },
    }));
    await expect(result).rejects.toMatchObject({ code: "permission-denied" });
  });

  test("requests the selected input and returns the acquired stream", async () => {
    const stream = {} as MediaStream;
    let received: MediaStreamConstraints | null = null;
    const result = await requestMicrophoneStream(
      { deviceId: { exact: "microphone-a" } },
      environment({
        mediaDevices: {
          getUserMedia: async (constraints) => {
            received = constraints;
            return stream;
          },
          enumerateDevices: async () => [],
        },
      }),
    );
    expect(result).toBe(stream);
    expect(received).toEqual({ audio: { deviceId: { exact: "microphone-a" } } });
  });

  test("does not require device enumeration to start capture", async () => {
    const stream = {} as MediaStream;
    await expect(requestMicrophoneStream(true, environment({
      mediaDevices: { getUserMedia: async () => stream },
    }))).resolves.toBe(stream);
  });

  test("does not require capture permission support to list devices", async () => {
    await expect(enumerateAudioInputDevices(environment({
      mediaDevices: { enumerateDevices: async () => [] },
    }))).resolves.toEqual([]);
  });

  test("classifies a security rejection from an insecure context as API unavailable", async () => {
    const result = requestMicrophoneStream(true, environment({
      secureContext: false,
      mediaDevices: {
        getUserMedia: async () => { throw new DOMException("Blocked", "SecurityError"); },
      },
    }));
    await expect(result).rejects.toMatchObject({ code: "api-unavailable" });
  });

  test("does not misreport a WebView security restriction as user denial", async () => {
    const result = requestMicrophoneStream(true, environment({
      mediaDevices: {
        getUserMedia: async () => { throw new DOMException("Blocked", "SecurityError"); },
      },
    }));
    await expect(result).rejects.toMatchObject({
      code: "api-unavailable",
      message: "Microphone capture is blocked by the current app or WebView security policy.",
    });
  });

  test("reports API absence while listing Settings devices", async () => {
    await expect(enumerateAudioInputDevices(environment({ mediaDevices: undefined })))
      .rejects.toMatchObject({
        name: "MicrophoneCaptureError",
        code: "api-unavailable",
        message: "Microphone device listing is unavailable in this SAAA build.",
      });
  });

  test("resumes a suspended Web Audio context before capture", async () => {
    const context = {
      state: "suspended" as AudioContextState,
      resume: async () => { context.state = "running"; },
    };
    await expect(ensureMicrophoneAudioContextRunning(context)).resolves.toBeUndefined();
    expect(context.state).toBe("running");
  });

  test("also resumes WebKit's non-standard interrupted state", async () => {
    const context = {
      state: "interrupted" as AudioContextState,
      resume: async () => { context.state = "running"; },
    };
    await expect(ensureMicrophoneAudioContextRunning(context)).resolves.toBeUndefined();
    expect(context.state).toBe("running");
  });

  test("fails instead of silently recording when Web Audio stays suspended", async () => {
    const context = {
      state: "suspended" as AudioContextState,
      resume: async () => undefined,
    };
    await expect(ensureMicrophoneAudioContextRunning(context)).rejects.toMatchObject({
      code: "startup-interrupted",
    });
  });
});
