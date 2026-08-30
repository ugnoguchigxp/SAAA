import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { meetingPreflight, transcribeAudioChunk } from "../src/lib/runtime";
describe("meeting contracts", () => {
  test("exposes the preflight command through the typed runtime boundary", () => {
    expect(typeof meetingPreflight).toBe("function");
  });
  test("keeps continuous Chat ASR separate from Meeting segment ASR", () => {
    const chat = ["../src/features/voice/useAmbientVoiceSession.ts", "../src/features/voice/voiceSegmentProcessor.ts"]
      .map((path) => readFileSync(new URL(path, import.meta.url), "utf8")).join("\n");
    const meeting = readFileSync(new URL("../src/features/meeting/useMeetingSession.ts", import.meta.url), "utf8");
    expect(chat).toContain("context.transcribe ?? transcribeAudio");
    expect(typeof transcribeAudioChunk).toBe("function");
    expect(chat).toContain("AmbientVoiceTranscriber");
    expect(readFileSync(new URL("../src/lib/voiceRuntime.ts", import.meta.url), "utf8")).toContain('"transcribe_audio_chunk"');
    expect(readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8")).toContain("transcribe_audio_chunk,");
    expect(meeting).toContain("appendMeetingAudioSegment({");
    expect(meeting).not.toContain("previewMeetingAudioSegment({");
  });
  test("requires an explicit review with persistence details before save or discard", () => {
    const source = readFileSync(new URL("../src/features/meeting/MeetingPage.tsx", import.meta.url), "utf8");
    for (const key of ["reviewSave", "targetValue", "audioValue", "confirmDiscard"]) expect(source).toContain(`t("meeting.${key}")`);
  });
});
