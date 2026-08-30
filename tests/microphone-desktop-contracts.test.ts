import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function chatVoiceSource(): string {
  return [
    readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8"),
    readFileSync(
      join(import.meta.dir, "../src/features/voice/useAmbientVoiceSession.ts"),
      "utf8",
    ),
    readFileSync(
      join(import.meta.dir, "../src/features/voice/ambientVoiceCapture.ts"),
      "utf8",
    ),
    readFileSync(join(import.meta.dir, "../src/features/voice/voiceAsrPacketizer.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/voiceAsrPacketSender.ts"), "utf8"),
    readFileSync(
      join(import.meta.dir, "../src/features/chat/ChatPage.tsx"),
      "utf8",
    ),
    readFileSync(
      join(import.meta.dir, "../src/features/chat/useConversationTurn.ts"),
      "utf8",
    ),
  ].join("\n");
}

describe("macOS microphone bundle configuration", () => {
  test("declares the purpose string and audio-input entitlement for signed builds", () => {
    const info = readFileSync(
      join(import.meta.dir, "../src-tauri/Info.plist"),
      "utf8",
    );
    const entitlements = readFileSync(
      join(import.meta.dir, "../src-tauri/Entitlements.plist"),
      "utf8",
    );
    const config = JSON.parse(
      readFileSync(
        join(import.meta.dir, "../src-tauri/tauri.conf.json"),
        "utf8",
      ),
    );
    expect(info).toContain("NSMicrophoneUsageDescription");
    expect(entitlements).toContain("com.apple.security.device.audio-input");
    expect(config.bundle.macOS.entitlements).toBe("Entitlements.plist");
  });

  test("verifies the packaged purpose string in desktop smoke", () => {
    const smoke = readFileSync(
      join(import.meta.dir, "../scripts/desktop-smoke.ts"),
      "utf8",
    );
    expect(smoke).toContain('"NSMicrophoneUsageDescription"');
    expect(smoke).toContain(
      "packaged Info.plist has no microphone usage description",
    );
  });

  test("routes every frontend microphone entry point through the checked boundary", () => {
    const app = chatVoiceSource();
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    const enrollment = readFileSync(
      join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"),
      "utf8",
    );
    const settings = [
      readFileSync(
        join(import.meta.dir, "../src/features/settings/SettingsPage.tsx"),
        "utf8",
      ),
      readFileSync(
        join(
          import.meta.dir,
          "../src/features/settings/VoiceSettingsSection.tsx",
        ),
        "utf8",
      ),
    ].join("\n");
    expect(app).toContain("requestMicrophoneStream(audio)");
    expect(meeting).toContain("requestMicrophoneStream(");
    expect(enrollment).toContain(
      "requestMicrophoneStream(microphoneCaptureConstraints(voice.inputDeviceId))",
    );
    expect(settings).toContain("enumerateAudioInputDevices()");
    expect(`${app}\n${meeting}\n${enrollment}\n${settings}`).not.toContain(
      "navigator.mediaDevices",
    );
  });

  test("keeps microphone processing constraints centralized", () => {
    const app = chatVoiceSource();
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    const enrollment = readFileSync(
      join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"),
      "utf8",
    );
    for (const source of [app, meeting, enrollment]) {
      expect(source).toContain("microphoneCaptureConstraints");
    }
    expect(
      readFileSync(join(import.meta.dir, "../src/lib/microphone.ts"), "utf8"),
    ).toContain("echoCancellation: false");
  });

  test("registers acquired streams before AudioContext construction can fail", () => {
    const app = chatVoiceSource();
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    expect(app.indexOf("context.stream.current = stream")).toBeLessThan(
      app.indexOf("audioContext = new AudioContext({ sampleRate: 16_000 })"),
    );
    expect(meeting.indexOf("stream.current = nextStream")).toBeLessThan(
      meeting.indexOf("const nextContext = new AudioContext()"),
    );
  });

  test("guards capture startup and finalization independently", () => {
    const app = chatVoiceSource();
    expect(app).toContain(
      "if (voiceSessionRef.current.actionInProgress) return",
    );
    expect(app).toContain("if (voiceSessionRef.current.finalizing)");
    expect(app).toContain(
      'applyVoiceEvent({ type: "finalizeRequested", mode })',
    );
    expect(app).toContain('applyEvent({ type: "captureStarting" })');
    expect(app).toContain("await finishVoiceCapture(false)");
    expect(app).toContain("void finishVoiceCapture(true)");
  });

  test("auto-finalizes each chat voice segment while keeping the microphone open", () => {
    const app = chatVoiceSource();
    const chatPage = readFileSync(
      join(import.meta.dir, "../src/features/chat/ChatPage.tsx"),
      "utf8",
    );
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    expect(app).toContain(
      "detector(context.settings, activeContext.sampleRate)",
    );
    expect(app).toContain(
      "new VoiceActivityDetector({ sampleRate, speechThresholdRms, silenceTimeoutMs:",
    );
    expect(app).toContain(
      "const observation = context.activityDetector.current?.observe(event.data)",
    );
    expect(app).toContain("observation.shouldFinalize");
    expect(app).toContain("context.packetFrame(event.data)");
    expect(app).toContain("VoiceAsrPacketizer");
    expect(app).toContain("VoiceAsrPacketSender");
    expect(app).toContain("voiceAsrPacketizerRef.current.append(frame)");
    expect(app).toContain("void finishVoiceCapture(true)");
    expect(app).toContain("await sender.enqueueCommit(\"silence\")");
    expect(chatPage).toContain('t("chat.listeningHint"');
    expect(chatPage).toContain('t("chat.micPause")');
    expect(chatPage).not.toContain("filterEnabled");
    expect(meeting).not.toContain("VoiceActivityDetector");
    expect(app).toContain("suspendVoiceForSpeech");
    expect(app).toContain("resumeVoiceAfterSpeech");
  });

  test("starts ambient listening automatically without blocking text or navigation", () => {
    const app = chatVoiceSource();
    const shell = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    expect(app).toContain("export function useAmbientVoiceSession");
    expect(app).toContain("void attachVoiceCapture()");
    expect(app).toContain("voiceSessionProcessing");
    expect(app).toContain("if (!enabled) void pauseAmbientCapture()");
    expect(app).toContain("|| !context.listeningEnabled.current");
    expect(app).toContain("restartCaptureForInputDeviceChange(inputDeviceId)");
    expect(app).toContain("voiceSettingsRef.current?.inputDeviceId !== inputDeviceId");
    expect(shell).toContain("voiceEnrollmentBlocked={voiceBusy");
    expect(shell).not.toContain("voiceBusy={voiceProcessing}");
    expect(app).not.toContain("allowVoiceBusy");
    expect(app).not.toContain("isVoiceBusy");
    expect(shell).not.toContain(
      "音声入力を停止してからSurfaceを切り替えてください。",
    );
    expect(app).toContain("context.stream.current || context.captureLease.current");
    expect(app).toContain("context.activityDetector.current === activityDetector");
    expect(app).toContain("context.captureLease.current === release");
    expect(app).toContain("if (context.node.current !== node) return");
    expect(app).toContain("voiceNodeRef.current.port.onmessage = null");
    expect(app).toContain('voiceStarting ? t("chat.micCancel")');
    expect(app).toContain('disabled={meetingActive}');
    expect(app).toContain('aria-pressed={listeningEnabled}');
    expect(app).toContain('disabled={!composer.trim() || !selectedConversation}');
  });

  test("hands microphone ownership to Meeting before preflight capture", () => {
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    expect(meeting.indexOf("await onBeforeCapture()")).toBeLessThan(
      meeting.indexOf(
        'acquireAudioCapture("meeting")',
        meeting.indexOf("async function start()"),
      ),
    );
    expect(meeting).toContain("node.current.port.onmessage = null");
  });

  test("keeps automatic voice turns connected to LLM submission and response speech", () => {
    const app = chatVoiceSource();
    expect(app).toContain(
      'void submitPrompt(queued.text, { inputOrigin: "voice" })',
    );
    expect(app).toContain(
      'pendingVoicePromptsRef.current.push({ content: queued.text, inputOrigin: "voice", sourceId: queued.utteranceId })',
    );
    expect(app).toContain("voiceSettings?.autoSpeak");
    expect(app).toContain("speechGateRef.current.accept(event)");
    expect(app).toContain("void startSpeech(finalSpeechText, conversationId)");
    expect(app).toContain("speechResumeTokenRef.current = speechRunId");
    expect(app).toContain("speechResumeTokenRef.current !== speechRunId");
  });

  test("stops future capture without discarding finalized transcription", () => {
    const app = chatVoiceSource();
    const pause = app.slice(
      app.indexOf("async function pauseAmbientCapture()"),
      app.indexOf("async function attachVoiceCapture()"),
    );
    expect(pause).toContain("await finishVoiceCapture(false)");
    expect(pause).not.toContain("cancelRun");
    expect(pause).not.toContain("voiceSegmentQueueRef.current.clear()");
    expect(app).toContain('void submitPrompt(queued.text, { inputOrigin: "voice" })');
  });

  test("sends chat PCM through the bounded raw ASR sender", () => {
    const app = chatVoiceSource();
    expect(app).toContain("voiceAsrPacketizerRef.current.append(frame)");
    expect(app).toContain("sender.enqueueAudio(packet)");
    expect(app).toContain("voiceAsrPacketizerRef.current.flushPadded()");
  });

  test("blocks chat capture while Meeting is still in preflight", () => {
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    expect(meeting).toContain(
      'applySnapshot({ ...snapshotRef.current, state: "preflight", error: null })',
    );
  });

  test("does not leave Meeting stuck in preflight when recovery lookup fails", () => {
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    expect(meeting).toContain(
      "const restored = await getMeetingSnapshot().catch(() => null)",
    );
    expect(meeting).toContain("applySnapshot(restored ?? idle)");
  });

  test("reconciles Meeting state after a post-start microphone failure", () => {
    const meeting = readFileSync(
      join(import.meta.dir, "../src/features/meeting/useMeetingSession.ts"),
      "utf8",
    );
    const recovery = meeting.slice(
      meeting.indexOf(
        "} catch (cause) {",
        meeting.indexOf("async function start()"),
      ),
      meeting.indexOf("} finally {", meeting.indexOf("async function start()")),
    );
    expect(recovery.indexOf("discardMeeting(startedSession)")).toBeLessThan(
      recovery.indexOf("getMeetingSnapshot()"),
    );
    expect(recovery).not.toContain(
      "if (startedSession) {\n        applySnapshot(idle)",
    );
  });
});
