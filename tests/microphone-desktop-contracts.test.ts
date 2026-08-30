import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function chatVoiceSource(): string {
  return [
    readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/usePushToTalk.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/chat/useConversationTurn.ts"), "utf8"),
  ].join("\n");
}

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
    const app = chatVoiceSource();
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    const enrollment = readFileSync(join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"), "utf8");
    const settings = readFileSync(join(import.meta.dir, "../src/features/settings/SettingsPage.tsx"), "utf8");
    expect(app).toContain("requestMicrophoneStream(audio)");
    expect(meeting).toContain("requestMicrophoneStream(");
    expect(enrollment).toContain("requestMicrophoneStream(microphoneCaptureConstraints(voice.inputDeviceId))");
    expect(settings).toContain("enumerateAudioInputDevices()");
    expect(`${app}\n${meeting}\n${enrollment}\n${settings}`).not.toContain("navigator.mediaDevices");
  });

  test("keeps microphone processing constraints centralized", () => {
    const app = chatVoiceSource();
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    const enrollment = readFileSync(join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"), "utf8");
    for (const source of [app, meeting, enrollment]) {
      expect(source).toContain("microphoneCaptureConstraints");
    }
    expect(readFileSync(join(import.meta.dir, "../src/lib/microphone.ts"), "utf8")).toContain("echoCancellation: true");
  });

  test("registers acquired streams before AudioContext construction can fail", () => {
    const app = chatVoiceSource();
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    expect(app.indexOf("voiceStreamRef.current = stream")).toBeLessThan(app.indexOf("const context = new AudioContext()"));
    expect(meeting.indexOf("stream.current = nextStream")).toBeLessThan(meeting.indexOf("const nextContext = new AudioContext()"));
  });

  test("guards capture startup and finalization independently", () => {
    const app = chatVoiceSource();
    expect(app).toContain("if (voiceSessionRef.current.actionInProgress) return");
    expect(app).toContain("if (voiceSessionRef.current.finalizing)");
    expect(app).toContain('applyVoiceEvent({ type: "finalizeRequested", mode })');
    expect(app).toContain('applyVoiceEvent({ type: "captureStarting" })');
    expect(app).toContain("void finishVoiceCapture(false)");
    expect(app).toContain("void finishVoiceCapture(true)");
  });

  test("auto-finalizes each chat voice segment while keeping the microphone open", () => {
    const app = chatVoiceSource();
    const chatPage = readFileSync(join(import.meta.dir, "../src/features/chat/ChatPage.tsx"), "utf8");
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
    expect(app).toContain("new VoiceActivityDetector({ sampleRate: context.sampleRate })");
    expect(app).toContain("const observation = voiceActivityDetectorRef.current?.observe(event.data)");
    expect(app).toContain("observation?.shouldFinalize");
    expect(app).toContain("void finishVoiceCapture(true)");
    expect(app).toContain("if (keepListening && voiceContextRef.current)");
    expect(chatPage).toContain('(voiceState === "idle" && Boolean(activeRunId) && !filterEnabled)');
    expect(chatPage).toContain("マイクは停止するまで待ち受け続けます。");
    expect(meeting).not.toContain("VoiceActivityDetector");
    expect(app).toContain("suspendVoiceForSpeech");
    expect(app).toContain("resumeVoiceAfterSpeech");
  });

  test("keeps automatic voice turns connected to LLM submission and response speech", () => {
    const app = chatVoiceSource();
    expect(app).toContain('void submitPrompt(transcript, { allowVoiceBusy: true, inputOrigin: "voice" })');
    expect(app).toContain('pendingVoicePromptsRef.current.push({ content: transcript, inputOrigin: "voice" })');
    expect(app).toContain('voiceSettings?.autoSpeak');
    expect(app).toContain("void startSpeech(event.message.content, conversationId)");
  });

  test("treats a requested transcription stop as cancellation rather than failure", () => {
    const app = chatVoiceSource();
    expect(app).toContain("const voiceRunId = voiceSessionRef.current.transcriptionRunId");
    expect(app).toContain('applyVoiceEvent({ type: "transcriptionCancelRequested" })');
    expect(app).toContain("if (!voiceSessionRef.current.cancellationRequested &&");
    expect(app).toContain("if (voiceSessionRef.current.cancellationRequested || !transcript.trim()) continue");
  });

  test("releases raw chat PCM before waiting for the model response", () => {
    const app = chatVoiceSource();
    const clearFrames = app.indexOf("voiceFramesRef.current = []", app.indexOf("async function finishVoiceCapture"));
    const enqueueTranscript = app.indexOf("enqueueVoiceSegment({", clearFrames);
    expect(clearFrames).toBeGreaterThan(-1);
    expect(clearFrames).toBeLessThan(enqueueTranscript);
    expect(app).toContain("segment.samples = []");
  });

  test("blocks chat capture while Meeting is still in preflight", () => {
    const meeting = readFileSync(join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"), "utf8");
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
