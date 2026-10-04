import { containsSource } from "./sourceContract";
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

describe("macOS microphone bundle configuration", () => {
  test("verifies the packaged purpose string in desktop smoke", () => {
    const smoke = [
      readFileSync(join(import.meta.dir, "../scripts/desktop-smoke.ts"), "utf8"),
      readFileSync(join(import.meta.dir, "../scripts/macos-bundle-smoke.ts"), "utf8"),
    ].join("\n");
    expect(containsSource(smoke, '"NSMicrophoneUsageDescription"')).toBe(true);
    expect(containsSource(smoke, "packaged Info.plist has no microphone usage description")).toBe(
      true,
    );
    expect(containsSource(smoke, "packaged app identity does not match tauri.conf.json")).toBe(
      true,
    );
    expect(containsSource(smoke, "signed app has no audio-input entitlement")).toBe(true);
  });

  test("routes current microphone capture through the checked boundary", () => {
    const browser = readFileSync(
      join(import.meta.dir, "../src/lib/browserVoiceCapture.ts"),
      "utf8",
    );
    expect(browser).toContain("requestMicrophoneStream(");
    expect(browser).toContain("disposeMicrophoneCapture(stream, context)");
    expect(browser).toContain("ensureMicrophoneAudioContextRunning(context)");
    expect(browser).toContain("event.data.fill(0)");
    expect(browser).toContain('acquireAudioCapture("chat")');
  });
  test("continuous ASR drains finalized input and releases its session on stop", () => {
    const capture = readFileSync(
      join(import.meta.dir, "../src/lib/conversationAsrCapture.ts"),
      "utf8",
    );
    expect(capture).toContain("await startCompletion");
    expect(capture).toContain("if (currentUtteranceId) finalizeUtterance()");
    expect(capture).toContain("await Promise.all([finalCompletion, partialCompletion])");
    expect(capture).toContain("await releaseConversationAsrSession()");
    expect(capture).not.toContain("suspendVoiceForSpeech");
    expect(capture).not.toContain("resumeVoiceAfterSpeech");
  });
});
