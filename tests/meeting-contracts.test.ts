import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { meetingPreflight, previewAudio, previewMeetingAudioSegment } from "../src/lib/runtime";

describe("meeting contracts", () => {
  test("exposes the preflight command through the typed runtime boundary", () => {
    expect(typeof meetingPreflight).toBe("function");
  });

  test("exposes real ASR preview commands for chat and meeting partials", () => {
    expect(typeof previewAudio).toBe("function");
    expect(typeof previewMeetingAudioSegment).toBe("function");
  });

  test("requires an explicit review with persistence details before save or discard", () => {
    const source = readFileSync(join(import.meta.dir, "../src/features/meeting/MeetingPage.tsx"), "utf8");
    expect(source).toContain("Review before save");
    expect(source).toContain("SAAA local SQLite database");
    expect(source).toContain("Raw microphone audio is deleted and is not saved.");
    expect(source).toContain("Confirm discard");
  });
});
