import { chatVoiceSource } from "./chatVoiceSource";
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

  test("routes every frontend microphone entry point through the checked boundary", () => {
    const app = chatVoiceSource();
    const enrollment = readFileSync(
      join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"),
      "utf8",
    );
    const settings = [
      readFileSync(join(import.meta.dir, "../src/features/settings/SettingsPage.tsx"), "utf8"),
      readFileSync(
        join(import.meta.dir, "../src/features/settings/VoiceSettingsSection.tsx"),
        "utf8",
      ),
    ].join("\n");
    expect(containsSource(app, "requestMicrophoneStream(audio)")).toBe(true);
    expect(
      containsSource(
        enrollment,
        "requestMicrophoneStream(microphoneCaptureConstraints(voice.inputDeviceId))",
      ),
    ).toBe(true);
    expect(containsSource(settings, "enumerateAudioInputDevices()")).toBe(true);
    expect(containsSource(`${app}\n${enrollment}\n${settings}`, "navigator.mediaDevices")).toBe(
      false,
    );
  });

  test("keeps microphone processing constraints centralized", () => {
    const app = chatVoiceSource();
    const enrollment = readFileSync(
      join(import.meta.dir, "../src/features/settings/VoiceProfileCard.tsx"),
      "utf8",
    );
    for (const source of [app, enrollment]) {
      expect(containsSource(source, "microphoneCaptureConstraints")).toBe(true);
    }
    expect(
      containsSource(
        readFileSync(join(import.meta.dir, "../src/lib/microphone.ts"), "utf8"),
        "echoCancellation: true",
      ),
    ).toBe(true);
  });

  test("registers acquired streams before AudioContext construction can fail", () => {
    const app = chatVoiceSource();
    expect(app.indexOf("context.stream.current = stream")).toBeLessThan(
      app.indexOf("audioContext = new AudioContext({ sampleRate: 16_000 })"),
    );
  });

  test("guards capture startup and finalization independently", () => {
    const app = chatVoiceSource();
    expect(
      containsSource(app, "if (shouldEnable && voiceSessionRef.current.actionInProgress) {"),
    ).toBe(true);
    expect(
      app.indexOf("if (shouldEnable && voiceSessionRef.current.actionInProgress) {"),
    ).toBeLessThan(app.indexOf("const generation = ++voiceToggleGenerationRef.current"));
    expect(containsSource(app, "if (voiceSessionRef.current.finalizing)")).toBe(true);
    expect(containsSource(app, 'applyVoiceEvent({ type: "finalizeRequested", mode })')).toBe(true);
    expect(containsSource(app, 'applyEvent({ type: "captureStarting" })')).toBe(true);
    expect(containsSource(app, "await finishVoiceCapture(false)")).toBe(true);
    expect(containsSource(app, "void finishVoiceCapture(true, reason)")).toBe(true);
  });

  test("auto-finalizes each chat voice segment while keeping the microphone open", () => {
    const app = chatVoiceSource();
    const chatPage = readFileSync(
      join(import.meta.dir, "../src/features/chat/ChatPage.tsx"),
      "utf8",
    );
    expect(containsSource(app, "detector(context.settings, activeContext.sampleRate)")).toBe(true);
    expect(
      containsSource(
        app,
        "new VoiceActivityDetector({ sampleRate, speechThresholdRms, silenceTimeoutMs:",
      ),
    ).toBe(true);
    expect(
      containsSource(
        app,
        "const observation = context.activityDetector.current?.observe(event.data)",
      ),
    ).toBe(true);
    expect(
      containsSource(app, "voiceSegmentCommitReason(observation, context.packetCount())"),
    ).toBe(true);
    expect(containsSource(app, "observation?.hasSpeech && observation.shouldFinalize")).toBe(true);
    expect(containsSource(app, "context.packetFrame(event.data)")).toBe(true);
    expect(containsSource(app, "VoiceAsrPacketizer")).toBe(true);
    expect(containsSource(app, "VoiceAsrPacketSender")).toBe(true);
    expect(containsSource(app, "voiceAsrPacketizerRef.current.append(frame)")).toBe(true);
    expect(containsSource(app, "void finishVoiceCapture(true, reason)")).toBe(true);
    expect(containsSource(app, "const commit = sender.enqueueCommit(reason)")).toBe(true);
    expect(app.indexOf("voiceAsrPacketCountRef.current = 0")).toBeLessThan(
      app.indexOf("await commit"),
    );
    expect(containsSource(chatPage, 't("chat.listeningHint"')).toBe(true);
    expect(containsSource(chatPage, 't("chat.micPause")')).toBe(true);
    expect(containsSource(chatPage, "filterEnabled")).toBe(false);
    expect(containsSource(app, "suspendVoiceForSpeech")).toBe(true);
    expect(containsSource(app, "resumeVoiceAfterSpeech")).toBe(true);
  });

  test("starts ambient listening automatically without blocking text or navigation", () => {
    const app = chatVoiceSource();
    const shell = readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8");
    expect(containsSource(app, "export function useAmbientVoiceSession")).toBe(true);
    expect(containsSource(app, "void attachVoiceCapture()")).toBe(true);
    expect(containsSource(app, "voiceSessionProcessing")).toBe(true);
    expect(containsSource(app, "if (!enabled) void pauseAmbientCaptureCommitted(false)")).toBe(
      true,
    );
    expect(containsSource(app, "|| !context.listeningEnabled.current")).toBe(true);
    expect(containsSource(app, "restartCaptureForInputDeviceChangeCommitted(inputDeviceId)")).toBe(
      true,
    );
    expect(containsSource(app, "voiceSettingsRef.current?.inputDeviceId !== inputDeviceId")).toBe(
      true,
    );
    expect(containsSource(shell, "voiceEnrollmentBlocked={voiceBusy")).toBe(true);
    expect(containsSource(shell, "voiceBusy={voiceProcessing}")).toBe(false);
    expect(containsSource(app, "allowVoiceBusy")).toBe(false);
    expect(containsSource(app, "isVoiceBusy")).toBe(false);
    expect(containsSource(shell, "音声入力を停止してからSurfaceを切り替えてください。")).toBe(
      false,
    );
    expect(containsSource(app, "context.stream.current || context.captureLease.current")).toBe(
      true,
    );
    expect(containsSource(app, "context.activityDetector.current === activityDetector")).toBe(true);
    expect(containsSource(app, "context.captureLease.current === release")).toBe(true);
    expect(containsSource(app, "if (context.node.current !== node) return")).toBe(true);
    expect(containsSource(app, "voiceNodeRef.current.port.onmessage = null")).toBe(true);
    expect(containsSource(app, 'voiceState === "preparing"')).toBe(true);
    expect(containsSource(app, "disabled={meetingActive}")).toBe(false);
    expect(containsSource(app, "aria-pressed={listeningEnabled}")).toBe(true);
    expect(containsSource(app, "disabled={!composer.trim() || !selectedConversation}")).toBe(true);
  });

  test("requests first-use permission from the user action and persists pause/resume immediately", () => {
    const app = chatVoiceSource();
    const toggle = app.slice(
      app.indexOf("async function toggleAmbientListening("),
      app.indexOf("async function restartCaptureForInputDeviceChange"),
    );
    expect(toggle.indexOf("requestMicrophoneStream(")).toBeLessThan(
      toggle.indexOf("persistListeningEnabled(true)"),
    );
    expect(containsSource(toggle, "persistListeningEnabled(false)")).toBe(true);
    expect(containsSource(app, "setVoiceListeningEnabled(enabled)")).toBe(true);
  });

  test("closes ASR after microphone startup failure and retries one stale session once", () => {
    const app = chatVoiceSource();
    const attach = app.slice(
      app.indexOf("async function attachVoiceCapture()"),
      app.indexOf("async function suspendVoice"),
    );
    expect(containsSource(attach, 'toMessage(cause) !== "asr-session-exists"')).toBe(true);
    expect(containsSource(attach, "await start(true)")).toBe(true);
    expect(containsSource(attach, "await detachVoiceCapture(false)")).toBe(true);
    expect(containsSource(attach, 'applyVoiceEvent({ type: "captureDetached" })')).toBe(true);
    expect(attach.indexOf("voiceAsrSessionIdRef.current = sessionId")).toBeLessThan(
      attach.indexOf("await start(false)"),
    );
    expect(containsSource(attach, "voiceAsrSessionIdRef.current !== sessionId")).toBe(true);
    expect(containsSource(attach, "!acceptedVoiceAsrSessionsRef.current.has(sessionId)")).toBe(
      true,
    );
  });

  test("resolves stop barriers even after a conversation rejects the old session", () => {
    const app = chatVoiceSource();
    const handler = app.slice(
      app.indexOf("function handleVoiceAsrEvent("),
      app.indexOf("async function terminateFailedVoiceCapture"),
    );
    expect(
      handler.indexOf("voiceAsrStopWaitersRef.current.get(event.sessionId)?.resolve()"),
    ).toBeLessThan(handler.indexOf("acceptedVoiceAsrSessionsRef.current.has(event.sessionId)"));
  });

  test("keeps automatic voice turns connected directly to LLM submission and response speech", () => {
    const app = chatVoiceSource();
    expect(containsSource(app, "receiveLfmUtterance(conversationId")).toBe(false);
    expect(containsSource(app, "speakLfmReply(conversationId")).toBe(false);
    expect(containsSource(app, "void submitPrompt(queued.text")).toBe(true);
    expect(containsSource(app, "sourceId: queued.utteranceId")).toBe(true);
    expect(containsSource(app, "voiceSettings?.autoSpeak")).toBe(true);
    expect(containsSource(app, 'case "speechStarted":')).toBe(true);
    expect(containsSource(app, 'case "speechEnded":')).toBe(true);
    expect(containsSource(app, 'type: "speechStarted", runId: event.runId')).toBe(true);
    expect(containsSource(app, 'type: "speechFinished", runId: event.runId')).toBe(true);
    expect(containsSource(app, "speechResumeTokenRef.current = speechRunId")).toBe(true);
    expect(containsSource(app, "speechResumeTokenRef.current !== speechRunId")).toBe(true);
  });

  test("stops future capture without discarding finalized transcription", () => {
    const app = chatVoiceSource();
    const pause = app.slice(
      app.indexOf("async function pauseAmbientCapture("),
      app.indexOf("async function attachVoiceCapture()"),
    );
    expect(containsSource(pause, "await finishVoiceCapture(false)")).toBe(true);
    expect(containsSource(pause, "cancelRun")).toBe(false);
    expect(containsSource(pause, "voiceSegmentQueueRef.current.clear()")).toBe(false);
    expect(containsSource(app, "void submitPrompt(queued.text")).toBe(true);
    expect(containsSource(app, "receiveLfmUtterance(conversationId")).toBe(false);
  });

  test("sends chat PCM through the bounded raw ASR sender", () => {
    const app = chatVoiceSource();
    expect(containsSource(app, "voiceAsrPacketizerRef.current.append(frame)")).toBe(true);
    expect(containsSource(app, "sender.enqueueAudio(packet)")).toBe(true);
    expect(containsSource(app, "voiceAsrPacketizerRef.current.flushPadded()")).toBe(true);
    expect(containsSource(app, "event.data.fill(0)")).toBe(true);
  });
});
