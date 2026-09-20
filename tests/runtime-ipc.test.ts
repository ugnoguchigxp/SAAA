import receiverFixtures from "./fixtures/ipc-receivers.json";
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { markReasoningRun } from "../src/lib/reasoningRun";

const {
  backupDatabase,
  cancelRun,
  deleteProviderApiKey,
  deleteVoiceEnrollmentSample,
  deleteVoiceProfile,
  exportDiagnostics,
  getAppSnapshot,
  getProviderCredentialState,
  getVoiceProfileSnapshot,
  listAuditEvents,
  listMessages,
  readVoiceEnrollmentSample,
  reportFrontendReady,
  reportOwnedSignal,
  resolveServiceHarness,
  saveSettingsDocuments,
  saveVoiceEnrollmentSample,
  setProviderApiKey,
  setTargetSpeakerFilterEnabled,
  setVoiceListeningEnabled,
  startTurn,
  stopTts,
  testModelProvider,
} = await import("../src/lib/runtime");
const { appendVoiceAsrAudio, commitVoiceAsrUtterance, startVoiceAsrSession, stopVoiceAsrSession } =
  await import("../src/lib/voiceAsrRuntime");
const { getConversationVoicePolicy, resetConversationVoicePolicy, updateConversationVoicePolicy } =
  await import("../src/lib/voiceBehaviorRuntime");
const { recordAuditEvent } = await import("../src/lib/auditRuntime");
const {
  currentLarmVoice,
  endLarmVoice,
  failLarmVoiceSession,
  ownLarmVoice,
  prepareLarmVoiceSession,
} = await import("../src/lib/larmVoiceRuntime");
const { cancelReasoningRun } = await import("../src/lib/reasoningRunControl");
const { stageAudioUpload } = await import("../src/lib/audioIpc");
const { notifyUiHistoryChanged, uiApi } = await import("../src/features/chat/ui/api");

function settingsDocuments() {
  return [
    {
      namespace: "providers.model",
      key: "default",
      schemaVersion: 15,
      valueJson: {
        harness: { address: "http://localhost:9810" },
        providers: [
          {
            kind: "openai-compatible",
            id: "local",
            enabled: true,
            label: "Local",
            location: "local",
            endpoint: "http://127.0.0.1:11434/v1",
            model: "test",
            authentication: "none",
          },
        ],
        reasoningEffort: "medium",
      },
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schemaVersion: 15,
      valueJson: {
        agentName: "SAAA",
        userName: "",
        enabled: false,
        provider: "codex-sdk",
        model: "",
        runtimeMode: "app-server",
        health: "unchecked",
        sandboxMode: "read-only",
        approvalPolicy: "never",
        networkEnabled: false,
        webSearchEnabled: false,
        workspacePolicy: "select-per-conversation",
      },
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 15,
      valueJson: {
        conversationRespond: {
          source: "provider",
          primaryProviderId: "local",
          fallbackProviderIds: [],
          timeoutMs: 30_000,
        },
        voiceTranscribe: { source: "harness", providerId: null, timeoutMs: 120_000 },
        voiceSpeak: { source: "harness", providerId: null, timeoutMs: 30_000 },
        codingAssist: {
          providerId: "codex-sdk",
          timeoutMs: 120_000,
          readOnly: true,
          networkEnabled: false,
          webSearchEnabled: false,
        },
      },
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schemaVersion: 15,
      valueJson: {
        listeningEnabled: true,
        inputDeviceId: "default",
        outputDeviceId: "default",
        vadSensitivity: "medium",
        silenceTimeoutMs: 1500,
        allowedLanguages: ["ja"],
        autoSpeak: true,
      },
    },
    {
      namespace: "security.runtime",
      key: "default",
      schemaVersion: 15,
      valueJson: { localOnlyWhenSelected: true, diagnosticsRedaction: true },
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schemaVersion: 15,
      valueJson: {
        enabled: false,
        sampleIntervalMs: 2_000,
        calendarEnabled: false,
        retentionDays: 7,
        maxLedgerEntries: 10_000,
        heartbeatIntervalMs: 300_000,
        sensitiveApplicationCategories: true,
      },
    },
    {
      namespace: "ui.preferences",
      key: "default",
      schemaVersion: 15,
      valueJson: {
        language: "system",
        timeZone: "system",
        lengthUnit: "metric",
        weightUnit: "kilogram",
        currency: "JPY",
      },
    },
  ];
}

function ensureWindow() {
  if (
    typeof globalThis.window !== "undefined" &&
    typeof globalThis.window.addEventListener === "function"
  )
    return;
  Object.assign(globalThis, { window: new EventTarget() });
}

beforeEach(() => {
  resetTauriCoreMock();
  ensureWindow();
});

afterEach(async () => {
  const current = currentLarmVoice();
  if (current) await endLarmVoice(current).catch(() => undefined);
});

describe("frontend IPC wrappers", () => {
  test("forwards conversation, settings, situation, and meeting commands", async () => {
    const events: unknown[] = [];
    await startTurn(
      {
        runId: "run-1",
        conversationId: "c1",
        content: "hello",
        workspacePath: null,
        inputOrigin: "text",
        presentationMode: "visual",
      },
      (event) => events.push(event),
    );
    await cancelRun("run-1");
    await testModelProvider({
      kind: "openai-compatible",
      id: "local",
      enabled: true,
      label: "Local",
      location: "local",
      endpoint: "http://127.0.0.1:11434/v1",
      model: "test",
      authentication: "none",
    });
    await stopTts("run-1");
    invokeImpl.handler = async (command) =>
      command === "get_app_snapshot" ? receiverFixtures.snapshot : command;
    await getAppSnapshot();
    await getVoiceProfileSnapshot();
    await setTargetSpeakerFilterEnabled(true);
    await deleteVoiceEnrollmentSample("sample-1");
    await deleteVoiceProfile();
    await readVoiceEnrollmentSample("sample-1");
    await reportFrontendReady();
    await exportDiagnostics();
    await listAuditEvents();
    await backupDatabase();
    await saveSettingsDocuments(settingsDocuments());
    await setVoiceListeningEnabled(false);
    await listMessages("c1", null);
    await reportOwnedSignal({
      conversationState: "idle",
      microphoneState: "inactive",
      audioState: "silent",
    });

    const names = invokeCalls.map((call) => call.command);
    expect(names).toContain("start_turn");
    expect(names).toContain("save_settings_documents");
    expect(names).not.toContain("watch_meeting");
    expect(names).not.toContain("get_situation_snapshot");
    expect(events).toEqual([]);
  });

  test("stages enrollment audio before invoking", async () => {
    invokeImpl.handler = async (command) =>
      command === "stage_audio_upload" ? "upload-1" : { id: command };
    const enrollment = new Float32Array([0.1, -0.2]);
    await saveVoiceEnrollmentSample({
      samples: enrollment,
      sampleRate: 16_000,
      inputDeviceId: "default",
      effectiveAec: false,
    });
    expect(enrollment[0]).toBe(0);
    expect(invokeCalls.map((call) => call.command)).toEqual([
      "stage_audio_upload",
      "save_voice_enrollment_sample",
    ]);
  });

  test("covers provider, voice policy, ASR, audit, and generative UI commands", async () => {
    await resolveServiceHarness("http://127.0.0.1:9810");
    await setProviderApiKey("local", "secret");
    await deleteProviderApiKey("local");
    await getProviderCredentialState("local");
    await getConversationVoicePolicy("c1");
    await updateConversationVoicePolicy({
      conversationId: "c1",
      speechOutput: "muted",
      listeningPace: null,
      expectedRevision: 1,
    });
    await resetConversationVoicePolicy({ conversationId: "c1", expectedRevision: 2 });
    await startVoiceAsrSession(
      { sessionId: "asr-1", conversationId: "c1", sampleRate: 16_000 },
      () => undefined,
    );
    await appendVoiceAsrAudio("asr-1", 1, new Uint8Array([1, 2]));
    await commitVoiceAsrUtterance({ sessionId: "asr-1", reason: "silence" });
    await stopVoiceAsrSession({ sessionId: "asr-1", finalizeCurrent: true });
    recordAuditEvent({ component: "frontend", eventName: "ready", phase: "start" });
    await uiApi.enabled();
    await uiApi.setEnabled(true);
    await uiApi.load("instance-1");
    await uiApi.query("instance-1", "runtime.summary");
    await uiApi.state("instance-1", 0, {});
    await uiApi.save("instance-1", "name", "description");
    await uiApi.archive("view-1");
    await uiApi.search("query");
    await uiApi.open("c1", "view-1");
    await uiApi.snapshot("instance-1");
    await uiApi.cancel("instance-1", "run-1", "req-1");
    const events: string[] = [];
    window.addEventListener("saaa:ui-history", (event) => {
      events.push((event as CustomEvent<string>).detail);
    });
    notifyUiHistoryChanged("c1");
    expect(events).toEqual(["c1"]);
    expect(invokeCalls.some((call) => call.command === "start_voice_asr_session")).toBe(true);
  });

  test("releases a LARM voice owner after a non-cancelled ASR start failure", async () => {
    ownLarmVoice("c2");
    invokeImpl.handler = async (command) => {
      if (command === "start_voice_asr_session") throw new Error("asr-provider-unavailable");
      return command;
    };
    await expect(
      startVoiceAsrSession(
        {
          sessionId: "asr-2",
          conversationId: "c2",
          sampleRate: 16_000,
        },
        () => undefined,
      ),
    ).rejects.toThrow("asr-provider-unavailable");
    expect(invokeCalls.map((call) => call.command)).toContain("end_larm_voice_session");
  });

  test("does not fail a LARM owner when ASR start is cancelled", async () => {
    invokeImpl.handler = async (command) => {
      if (command === "start_voice_asr_session") throw new Error("asr-cancelled");
      return command;
    };
    await expect(
      startVoiceAsrSession(
        {
          sessionId: "asr-3",
          conversationId: "c3",
          sampleRate: 16_000,
        },
        () => undefined,
      ),
    ).rejects.toThrow("asr-cancelled");
    expect(invokeCalls.filter((call) => call.command === "end_larm_voice_session")).toHaveLength(0);
  });

  test("owns, prepares, and ends a LARM voice session through the runtime adapters", async () => {
    const owner = ownLarmVoice("c4");
    expect(currentLarmVoice()?.id).toBe(owner.id);
    await prepareLarmVoiceSession("c4");
    await failLarmVoiceSession(null);
    await endLarmVoice(owner);
    expect(currentLarmVoice()).toBeNull();
  });

  test("cancels a marked reasoning run and restores cancellation state on failure", async () => {
    markReasoningRun("reason-1", "c1");
    await cancelReasoningRun(null);
    await cancelReasoningRun("reason-1");
    invokeImpl.handler = async () => {
      throw new Error("busy");
    };
    markReasoningRun("reason-2", "c1");
    await expect(cancelReasoningRun("reason-2")).rejects.toThrow("busy");
  });

  test("rejects empty staged audio and still clears PCM after a successful upload", async () => {
    await expect(stageAudioUpload(new Float32Array(), "voice-enrollment")).rejects.toThrow("empty");
    invokeImpl.handler = async () => "upload-2";
    const samples = new Float32Array([1]);
    expect(await stageAudioUpload(samples, "voice-enrollment")).toBe("upload-2");
  });
});
