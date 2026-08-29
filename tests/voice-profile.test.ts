import { afterEach, describe, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { acquireAudioCapture, currentAudioCaptureOwner } from "../src/lib/audioCaptureCoordinator";

afterEach(() => {
  if (currentAudioCaptureOwner()) {
    throw new Error("A test leaked an audio capture lease");
  }
});

describe("target-speaker voice profile", () => {
  test("serializes chat, meeting, and enrollment microphone ownership", () => {
    const release = acquireAudioCapture("chat");
    expect(currentAudioCaptureOwner()).toBe("chat");
    expect(() => acquireAudioCapture("voice-enrollment")).toThrow("already in use by chat");
    release();
    const releaseEnrollment = acquireAudioCapture("voice-enrollment");
    expect(currentAudioCaptureOwner()).toBe("voice-enrollment");
    releaseEnrollment();
    expect(currentAudioCaptureOwner()).toBeNull();
  });

  test("exposes enrollment lifecycle commands through the typed boundary", async () => {
    const runtime = await readFile(new URL("../src/lib/runtime.ts", import.meta.url), "utf8");
    for (const command of [
      "get_voice_profile_snapshot",
      "save_voice_enrollment_sample",
      "set_target_speaker_filter_enabled",
      "delete_voice_enrollment_sample",
      "delete_voice_profile",
      "read_voice_enrollment_sample",
    ]) expect(runtime).toContain(command);
  });

  test("bundles only checksum-pinned local verification artifacts", async () => {
    const installer = await readFile(new URL("../scripts/install-speaker-runtime.sh", import.meta.url), "utf8");
    const tauri = await readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8");
    const backend = await readFile(new URL("../src-tauri/src/voice/profile.rs", import.meta.url), "utf8");
    expect(installer).toContain("archive_sha=\"812b144d");
    expect(installer).toContain("model_sha=\"f682b514");
    expect(tauri).toContain('"resources/voice/": "voice/"');
    expect(backend).toContain("verify_if_enabled");
    expect(backend).toContain("TARGET_SPEAKER_REJECTED");
  });

  test("keeps filtered listening active while serializing ASR and LLM work", async () => {
    const app = await readFile(new URL("../src/App.tsx", import.meta.url), "utf8");
    expect(app).toContain("finishVoiceCapture(targetSpeakerFilterEnabledRef.current)");
    expect(app).toContain("if (keepListening && voiceContextRef.current)");
    expect(app).toContain("voiceSegmentQueueRef.current.length >= 2");
    expect(app).toContain("pendingVoicePromptsRef.current.length >= 2");
    expect(app).toContain("if (activeTtsRunIdRef.current) await stopSpeech()");
    expect(app).toContain("!targetSpeakerFilterEnabledRef.current");
  });
});
