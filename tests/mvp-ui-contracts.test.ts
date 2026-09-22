import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}
describe("MVP UI reachability contracts", () => {
  test("exposes one continuous normal conversation", () => {
    const app = source("src/App.tsx");
    const contracts = source("src/lib/contracts.ts");
    expect(contracts).toContain("primaryConversationId: string");
    expect(app).toContain("nextSnapshot.primaryConversationId");
    expect(app).toContain('useState<AppRoute>("conversation")');
    expect(app).not.toContain("新しい会話");
    expect(app).not.toContain("最近の会話");
    expect(app).not.toContain("primary-nav");
    expect(app).not.toContain("MeetingPage");
    expect(app).not.toContain("SituationPage");
    expect(app).toContain("AuditLogPage");
    expect(source("src/features/chat/ChatPage.tsx")).not.toContain("ChatOverflowMenu");
    expect(source("src/shell/appRoute.ts")).toContain('"audit"');
  });
  test("keeps normal Chat workspace-free", () => {
    const app =
      source("src/App.tsx") +
      source("src/features/chat/useConversationTurn.ts") +
      source("src/useOwnedSignalHeartbeat.ts");
    expect(app).toContain("workspacePath: null");
    expect(app).toContain('conversationState: activeRunId ? "model-running"');
    expect(app).not.toContain("workspacePath.trim()");
    expect(app).not.toContain("agent-running");
  });
  test("keeps the active run controls reachable while settings is requested", () => {
    const app = source("src/App.tsx");
    const navigate = app.slice(
      app.indexOf("function navigate("),
      app.indexOf("function openSettings()"),
    );
    expect(navigate).toContain('next === "settings" && !canChangeConversation()');
    expect(navigate).toContain("setRoute(next)");
    expect(app).toContain("onOpenSettings={openSettings}");
    expect(app).not.toContain("音声入力を停止してからSurfaceを切り替えてください。");
  });
  test("removes Codex controls from Settings while preserving the stored document", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    const settingsPersistence = source("src/features/settings/settingsDraft.ts");
    expect(settings).not.toContain('id: "codex"');
    expect(settings).not.toContain("<CodexSection");
    expect(settings).not.toContain("coding.assist");
    expect(settings).not.toContain("getCodexStatus");
    expect(settings).not.toContain("listCodexModels");
    expect(settingsPersistence).toContain('document("providers.agent", "codex-sdk", {');
    expect(settingsPersistence).toContain("...draft.codex");
  });
  test("lets users configure conversation identity names in General settings", () => {
    const settings =
      source("src/features/settings/SettingsPage.tsx") +
      source("src/features/settings/SettingsGeneralSection.tsx");
    const defaults = source("src/features/settings/settingsDefaults.ts");
    const settingsPersistence = source("src/features/settings/settingsDraft.ts");
    expect(settings).toContain('t("settings.general.agentName")');
    expect(settings).toContain('t("settings.general.identity")');
    expect(settingsPersistence).toContain("agentName: draft.codex.agentName.trim()");
    expect(settings).toContain('t("settings.general.userName")');
    expect(defaults).toContain('userName: ""');
    expect(settings).toContain('t("settings.general.userNamePlaceholder")');
    expect(settingsPersistence).toContain("userName: draft.codex.userName.trim()");
  });
  test("uses one bounded final-segment path for voice transcription", () => {
    const contracts = source("src/lib/contracts.ts");
    const voice =
      source("src/features/voice/useAmbientVoiceSession.ts") +
      source("src/features/voice/ambientVoiceCaptureActions.ts");
    const transcriber = source("src/features/voice/voiceAsrPacketSender.ts");
    expect(contracts).not.toContain('type: "transcriptDelta"');
    expect(voice).toContain("packetVoiceFrame");
    expect(transcriber).toContain("enqueueAudio");
    expect(transcriber).toContain("this.operations.push");
    expect(contracts).not.toContain('type: "transcriptPartial"');
    expect(contracts).not.toContain("MeetingSnapshot");
    expect(source("src/App.tsx")).not.toContain("MeetingPage");
  });
});
