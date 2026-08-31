import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}
describe("MVP UI reachability contracts", () => {
  test("removes the Coding surface and always starts normal conversation turns", () => {
    const app =
      source("src/App.tsx") +
      source("src/features/chat/useConversationTurn.ts");
    expect(app).toContain(
      'type Surface = "chat" | "meeting" | "situation" | "audit" | "settings"',
    );
    expect(app).toContain("workspacePath: null");
    expect(app).not.toContain("openCodingSurface");
    expect(app).not.toContain("createCodingConversation");
    expect(app).not.toContain("Coding thread");
    expect(app).not.toContain("Codex ready");
  });
  test("exposes one continuous normal conversation", () => {
    const app = source("src/App.tsx");
    const contracts = source("src/lib/contracts.ts");
    expect(contracts).toContain("primaryConversationId: string");
    expect(app).toContain("nextSnapshot.primaryConversationId");
    expect(app).toContain("snapshot.primaryConversationId");
    expect(app).toContain('setSurface("chat")');
    expect(app).not.toContain("新しい会話");
    expect(app).not.toContain("最近の会話");
  });
  test("keeps normal Chat workspace-free and Meeting transitions safe", () => {
    const app =
      source("src/App.tsx") +
      source("src/features/chat/useConversationTurn.ts");
    expect(app).toContain("workspacePath: null");
    expect(app).toContain('conversationState: activeRunId ? "model-running"');
    expect(app).not.toContain("workspacePath.trim()");
    expect(app).not.toContain("agent-running");
    expect(app).toContain(
      "if (conversationSessionRef.current.speechRunId) await stopSpeech()",
    );
  });
  test("keeps the active run controls reachable while navigation is requested", () => {
    const app = source("src/App.tsx");
    expect(app).toContain(
      'function openAuxiliarySurface(nextSurface: "settings" | "situation" | "audit")',
    );
    expect(app).toContain("if (!canChangeConversation()) return;");
    expect(app).toContain('openAuxiliarySurface("settings")');
    expect(app).toContain('openAuxiliarySurface("situation")');
    expect(app).not.toContain(
      "音声入力を停止してからSurfaceを切り替えてください。",
    );
  });
  test("removes Codex controls from Settings while preserving the stored document", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    const settingsPersistence = source(
      "src/features/settings/settingsDraft.ts",
    );
    expect(settings).not.toContain('id: "codex"');
    expect(settings).not.toContain("<CodexSection");
    expect(settings).not.toContain("coding.assist");
    expect(settings).not.toContain("getCodexStatus");
    expect(settings).not.toContain("listCodexModels");
    expect(settingsPersistence).toContain(
      'document("providers.agent", "codex-sdk", {',
    );
    expect(settingsPersistence).toContain("...draft.codex");
  });
  test("lets users configure conversation identity names in General settings", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    const defaults = source("src/features/settings/settingsDefaults.ts");
    const settingsPersistence = source(
      "src/features/settings/settingsDraft.ts",
    );
    expect(settings).toContain('t("settings.general.agentName")');
    expect(settings).toContain('t("settings.general.identity")');
    expect(settingsPersistence).toContain(
      "agentName: draft.codex.agentName.trim()",
    );
    expect(settings).toContain('t("settings.general.userName")');
    expect(defaults).toContain('userName: ""');
    expect(settings).toContain('t("settings.general.userNamePlaceholder")');
    expect(settingsPersistence).toContain(
      "userName: draft.codex.userName.trim()",
    );
  });
  test("transcribes captured audio continuously without treating chunks as final events", () => {
    const contracts = source("src/lib/contracts.ts");
    const voice = source("src/features/voice/useAmbientVoiceSession.ts");
    const transcriber = source("src/features/voice/voiceAsrPacketSender.ts");
    const meeting = source("src/features/meeting/useMeetingSession.ts");
    expect(contracts).not.toContain('type: "transcriptDelta"');
    expect(voice).toContain("packetVoiceFrame");
    expect(transcriber).toContain("enqueueAudio");
    expect(transcriber).toContain("this.operations.push");
    expect(contracts).toContain('type: "transcriptPartial"');
    expect(meeting).toContain(
      'event.type === "transcriptPartial" || event.type === "transcriptFinal"',
    );
  });
});
