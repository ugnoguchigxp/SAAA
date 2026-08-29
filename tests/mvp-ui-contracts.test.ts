import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("MVP UI reachability contracts", () => {
  test("exposes Coding as a separate read-only surface", () => {
    const app = source("src/App.tsx");
    expect(app).not.toContain("switchTaskMode");
    expect(app).toContain('type Surface = "chat" | "coding"');
    expect(app).toContain("openCodingSurface");
    expect(app).toContain("読み取り専用Codex workspaceを選択");
    expect(app).toContain("新しいCoding thread");
    expect(app).toContain("Codex ready");
    expect(app).toContain('workspacePath: runMode === "coding" ? workspacePath.trim() : null');
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

  test("keeps normal Chat workspace-free and Coding read-only", () => {
    const app = source("src/App.tsx");
    const contracts = source("src/lib/contracts.ts");
    expect(app).toContain('workspacePath: runMode === "coding" ? workspacePath.trim() : null');
    expect(app).toContain('activeRunMode === "coding" ? "agent-running" : "model-running"');
    expect(contracts).toContain('sandboxMode: "read-only"');
    expect(contracts).toContain("networkEnabled: false");
    expect(contracts).toContain("webSearchEnabled: false");
    expect(app).toContain("Meeting中はCoding Agentを開始できません");
    expect(app).toContain("if (activeTtsRunIdRef.current || streamingSpeechSessionRef.current) await stopSpeech()");
  });

  test("keeps the active run controls reachable while navigation is requested", () => {
    const app = source("src/App.tsx");
    expect(app).toContain('function openAuxiliarySurface(nextSurface: "settings" | "situation")');
    expect(app).toContain("if (!canChangeConversation()) return;");
    expect(app).toContain('openAuxiliarySurface("settings")');
    expect(app).toContain('openAuxiliarySurface("situation")');
  });

  test("exposes fixed read-only Codex configuration in Settings", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).toContain('id: "codex"');
    expect(settings).toContain("<CodexSection");
    expect(settings).toContain("coding.assist");
    expect(settings).toContain("getCodexStatus()");
    expect(settings).toContain('<Policy label="Sandbox" value={codex.sandboxMode} />');
    expect(settings).toContain('<Policy label="Network" value="disabled" />');
  });

  test("lets users select the conversation reasoning effort in LLM Providers", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).toContain('Field label="Reasoning effort"');
    expect(settings).toContain('<option value="low">Low</option>');
    expect(settings).toContain('<option value="medium">Medium (recommended)</option>');
    expect(settings).toContain('<option value="xhigh">Extra high</option>');
  });

  test("configures gnosis by host and resolves provider details dynamically", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    const gnosis = source("src-tauri/src/providers/gnosis.rs");
    expect(settings).toContain('Field label="LLM host server"');
    expect(settings).toContain("ホスト名またはプライベートIPだけを入力します");
    expect(settings).toContain("モデル・Gateway URL・短期credentialを接続APIから動的に解決");
    expect(settings).toContain("保存対象はhostのみ");
    expect(settings).toContain('`http://${provider.host || "<host>"}:9810`');
    expect(gnosis).toContain('format!("http://{host}:{CONTROL_PORT}/")');
    expect(gnosis).not.toContain('Command::new("ssh")');
    expect(gnosis).toContain('.join("v1/agent-profiles")');
    expect(gnosis).toContain('.extend(["v1", "agent-connections", id])');
    expect(gnosis).toContain('.push("claim")');
    expect(gnosis).toContain('"openai-provider-v1"');
  });

  test("renders actual partial and final transcript events", () => {
    const contracts = source("src/lib/contracts.ts");
    const meeting = source("src/features/meeting/useMeetingSession.ts");
    expect(contracts).toContain('type: "transcriptDelta"');
    expect(contracts).toContain('type: "transcriptPartial"');
    expect(meeting).toContain('event.type === "transcriptPartial" || event.type === "transcriptFinal"');
  });
});
