import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { meetingPreflight } from "../src/lib/runtime";
describe("meeting contracts", () => {
  test("exposes the preflight command through the typed runtime boundary", () => {
    expect(typeof meetingPreflight).toBe("function");
  });
  test("submits each captured segment to ASR only once", () => {
    const chat = ["../src/features/voice/useAmbientVoiceSession.ts", "../src/features/voice/voiceSegmentProcessor.ts"]
      .map((path) => readFileSync(new URL(path, import.meta.url), "utf8")).join("\n");
    const meeting = readFileSync(new URL("../src/features/meeting/useMeetingSession.ts", import.meta.url), "utf8");
    expect(chat).toContain("transcribeAudio({");
    expect(chat).not.toContain("previewAudio({");
    expect(readFileSync(new URL("../src/lib/runtime.ts", import.meta.url), "utf8")).not.toContain("preview_audio");
    expect(readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8")).not.toContain("preview_audio");
    expect(meeting).toContain("appendMeetingAudioSegment({");
    expect(meeting).not.toContain("previewMeetingAudioSegment({");
  });
  test("requires an explicit review with persistence details before save or discard", () => {
    const source = readFileSync(new URL("../src/features/meeting/MeetingPage.tsx", import.meta.url), "utf8");
    expect(source).toContain("Review before save");
    expect(source).toContain("SAAA local SQLite database");
    expect(source).toContain("Raw microphone audio is deleted and is not saved.");
    expect(source).toContain("Confirm discard");
  });
});
