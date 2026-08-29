import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("MVP UI reachability contracts", () => {
  test("removes the Coding surface and always starts normal conversation turns", () => {
    const app = source("src/App.tsx") + source("src/features/chat/useConversationTurn.ts");
    expect(app).toContain('type Surface = "chat" | "meeting" | "situation" | "settings"');
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
    const app = source("src/App.tsx") + source("src/features/chat/useConversationTurn.ts");
    expect(app).toContain("workspacePath: null");
    expect(app).toContain('conversationState: activeRunId ? "model-running"');
    expect(app).not.toContain("workspacePath.trim()");
    expect(app).not.toContain("agent-running");
    expect(app).toContain("if (activeTtsRunIdRef.current || streamingSpeechSessionRef.current) await stopSpeech()");
  });

  test("keeps the active run controls reachable while navigation is requested", () => {
    const app = source("src/App.tsx");
    expect(app).toContain('function openAuxiliarySurface(nextSurface: "settings" | "situation")');
    expect(app).toContain("if (!canChangeConversation()) return;");
    expect(app).toContain('openAuxiliarySurface("settings")');
    expect(app).toContain('openAuxiliarySurface("situation")');
  });

  test("removes Codex controls from Settings while preserving the stored document", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).not.toContain('id: "codex"');
    expect(settings).not.toContain("<CodexSection");
    expect(settings).not.toContain("coding.assist");
    expect(settings).not.toContain("getCodexStatus");
    expect(settings).not.toContain("listCodexModels");
    expect(settings).toContain('document("providers.agent", "codex-sdk", draft.codex)');
  });

  test("lets users select the conversation reasoning effort in LLM Providers", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).toContain('Field label="Reasoning effort"');
    expect(settings).toContain('<option value="low">Low</option>');
    expect(settings).toContain('<option value="medium">Medium (recommended)</option>');
    expect(settings).toContain('<option value="xhigh">Extra high</option>');
  });

  test("configures dynamic_lan by host and resolves provider details dynamically", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    const dynamic_lan = [
      source("src-tauri/src/providers/dynamic_lan/mod.rs"),
      source("src-tauri/src/providers/dynamic_lan/http.rs"),
      source("src-tauri/src/providers/dynamic_lan/validate.rs"),
    ].join("\n");
    expect(settings).toContain('Field label="LLM host server"');
    expect(settings).toContain("ホスト名またはプライベートIPだけを入力します");
    expect(settings).toContain("モデル・Gateway URL・短期credentialを接続APIから動的に解決");
    expect(settings).toContain("保存対象はhostのみ");
    expect(settings).toContain('`http://${provider.host || "<host>"}:9810`');
    expect(dynamic_lan).toContain('format!("http://{host}:{CONTROL_PORT}/")');
    expect(dynamic_lan).not.toContain('Command::new("ssh")');
    expect(dynamic_lan).toContain('.join("v1/agent-profiles")');
    expect(dynamic_lan).toContain('.extend(["v1", "agent-connections", id])');
    expect(dynamic_lan).toContain('.push("claim")');
    expect(dynamic_lan).toContain('"openai-provider-v1"');
  });

  test("renders actual partial and final transcript events", () => {
    const contracts = source("src/lib/contracts.ts");
    const meeting = source("src/features/meeting/useMeetingSession.ts");
    expect(contracts).toContain('type: "transcriptDelta"');
    expect(contracts).toContain('type: "transcriptPartial"');
    expect(meeting).toContain('event.type === "transcriptPartial" || event.type === "transcriptFinal"');
  });
});
