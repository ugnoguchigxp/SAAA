import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  ensureMicrophoneAudioContextRunning,
  enumerateAudioInputDevices,
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

describe("macOS microphone bundle configuration", () => {
  test("declares the purpose string and audio-input entitlement for signed builds", () => {
    const info = readFileSync(join(import.meta.dir, "../src-tauri/Info.plist"), "utf8");
    const entitlements = readFileSync(join(import.meta.dir, "../src-tauri/Entitlements.plist"), "utf8");
    const config = JSON.parse(readFileSync(join(import.meta.dir, "../src-tauri/tauri.conf.json"), "utf8"));
    expect(info).toContain("NSMicrophoneUsageDescription");
    expect(entitlements).toContain("com.apple.security.device.audio-input");
    expect(config.bundle.macOS.entitlements).toBe("Entitlements.plist");
  });

  test("verifies the packaged purpose string in desktop smoke", () => {
    const smoke = readFileSync(join(import.meta.dir, "../scripts/desktop-smoke.ts"), "utf8");
    expect(smoke).toContain('"NSMicrophoneUsageDescription"');
    expect(smoke).toContain("packaged Info.plist has no microphone usage description");
  });

  test("routes every frontend microphone entry point through the checked boundary", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    const settings = readFileSync(join(import.meta.dir, "../src/features/settings/SettingsPage.tsx"), "utf8");
    expect(app).toContain("requestMicrophoneStream(audio)");
    expect(meeting).toContain("requestMicrophoneStream(");
    expect(settings).toContain("enumerateAudioInputDevices()");
    expect(`${app}\n${meeting}\n${settings}`).not.toContain("navigator.mediaDevices");
  });

  test("registers acquired streams before AudioContext construction can fail", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    expect(app.indexOf("voiceStreamRef.current = stream")).toBeLessThan(app.indexOf("const context = new AudioContext()"));
    expect(meeting.indexOf("stream.current = nextStream")).toBeLessThan(meeting.indexOf("const nextContext = new AudioContext()"));
  });

  test("guards capture startup and finalization independently", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    expect(app).toContain("if (voiceActionRef.current) return");
    expect(app).toContain("if (voiceFinalizingRef.current) return");
    expect(app).toContain("setVoiceStarting(true)");
    expect(app).toContain("void finishVoiceCapture()");
  });

  test("treats a requested transcription stop as cancellation rather than failure", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    expect(app).toContain("const voiceRunId = activeVoiceRunIdRef.current");
    expect(app).toContain("voiceCancellationRequestedRef.current = true");
    expect(app).toContain("if (!voiceCancellationRequestedRef.current)");
    expect(app).toContain("if (voiceCancellationRequestedRef.current) return");
  });

  test("releases raw chat PCM before waiting for the model response", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    const clearFrames = app.indexOf("voiceFramesRef.current = []", app.indexOf("async function finishVoiceCapture"));
    const submitTranscript = app.indexOf("await submitPrompt(transcript, true)");
    expect(clearFrames).toBeGreaterThan(-1);
    expect(clearFrames).toBeLessThan(submitTranscript);
    expect(app).toContain("samples = []");
  });

  test("blocks chat capture while Meeting is still in preflight", () => {
    const app = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    expect(app).toContain('return state === "preflight" || state === "active"');
    expect(meeting).toContain('applySnapshot({ ...snapshotRef.current, state: "preflight", error: null })');
  });

  test("does not leave Meeting stuck in preflight when recovery lookup fails", () => {
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    expect(meeting).toContain("const restored = await getMeetingSnapshot().catch(() => null)");
    expect(meeting).toContain("applySnapshot(restored ?? idle)");
  });

  test("reconciles Meeting state after a post-start microphone failure", () => {
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    const recovery = meeting.slice(meeting.indexOf("} catch (cause) {", meeting.indexOf("async function start()")), meeting.indexOf("} finally {", meeting.indexOf("async function start()")));
    expect(recovery.indexOf("discardMeeting(startedSession)")).toBeLessThan(recovery.indexOf("getMeetingSnapshot()"));
    expect(recovery).not.toContain("if (startedSession) {\n        applySnapshot(idle)");
  });
});
