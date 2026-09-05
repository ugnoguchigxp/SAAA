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
    const backend = [
      await readFile(new URL("../src-tauri/src/voice/profile/mod.rs", import.meta.url), "utf8"),
      await readFile(new URL("../src-tauri/src/voice/profile/codec.rs", import.meta.url), "utf8"),
      await readFile(new URL("../src-tauri/src/voice/profile/streaming_verifier.rs", import.meta.url), "utf8"),
    ].join("\n");
    expect(installer).toContain("archive_sha=\"812b144d");
    expect(installer).toContain("model_sha=\"f682b514");
    expect(tauri).toContain('"resources/voice/": "voice/"');
    expect(backend).toContain("prepare_streaming_verifier");
    expect(backend).toContain("TARGET_SPEAKER_REJECTED");
    expect(backend).not.toMatch(/security_framework|load_master_key|encrypt_payload|decrypt_payload/);
    for (const marker of ['format!("{sample_id}.wav")', "embedding BLOB NOT NULL", "migrate_v14_to_v15"]) expect(backend).toContain(marker);
  });

  test("keeps listening active while serializing ASR and LLM work", async () => {
    const modules = ["useAmbientVoiceSession.ts", "ambientVoiceCapture.ts", "voiceAsrPacketSender.ts"];
    const app = (await Promise.all(modules.map((file) => readFile(new URL(`../src/features/voice/${file}`, import.meta.url), "utf8")))).join("\n");
    expect(app).toContain("finishVoiceCapture(true, reason)");
    expect(app).toContain("const commit = sender.enqueueCommit(reason)");
    expect(app).toContain("await commit");
    expect(app).toContain("new VoiceAsrPacketSender");
    expect(app).toContain("voiceFinalDeliveryRef.current.push");
    expect(app).toContain("pendingVoicePromptsRef.current.length >= 2");
    expect(app).toContain("suspendVoiceForSpeech");
    expect(app).toContain("voiceAsrPacketizerRef.current.flushPadded()");
  });
});
