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
    expect(app).toContain("snapshot.primaryConversationId");
    expect(app).toContain('useState<AppRoute>("conversation")');
    expect(app).not.toContain("新しい会話");
    expect(app).not.toContain("最近の会話");
    expect(app).not.toContain("primary-nav");
    expect(app).not.toContain("MeetingPage");
    expect(app).not.toContain("SituationPage");
    expect(app).toContain("AuditLogPage");
    expect(source("src/features/chat/ConversationCheckPage.tsx")).not.toContain("ChatOverflowMenu");
    expect(source("src/shell/appRoute.ts")).toContain('"audit"');
  });
  test("keeps the current conversation mounted across navigation", () => {
    const app = source("src/App.tsx");
    expect(app).toContain('hidden={route !== "conversation"}');
    expect(app).toContain("onRouteChange={setRoute}");
    const page = source("src/features/chat/ConversationCheckPage.tsx");
    expect(page).toContain("enqueueConversationText(");
    expect(page).not.toContain("workspacePath");
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
  test("uses bounded final recognition and preserves PCM capture during playback", () => {
    const voice = source("src/lib/conversationAsrCapture.ts");
    expect(voice).toContain("MAX_PENDING_FINALS = 8");
    expect(voice).toContain("MAX_UTTERANCE_SAMPLES = SAMPLE_RATE * 30");
    expect(voice).toContain("queueConversationAsrDelivery");
    expect(voice).not.toContain("suspendVoiceForSpeech");
  });
});
