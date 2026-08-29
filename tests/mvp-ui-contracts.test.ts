import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("MVP UI reachability contracts", () => {
  test("does not expose Coding assist controls in the chat UI", () => {
    const app = source("src/App.tsx");
    expect(app).not.toContain("switchTaskMode");
    expect(app).not.toContain("chooseWorkspace");
    expect(app).not.toContain("Coding thread");
    expect(app).not.toContain("Codex ready");
  });

  test("exposes one continuous normal conversation", () => {
    const app = source("src/App.tsx");
    const contracts = source("src/lib/contracts.ts");
    expect(contracts).toContain("primaryConversationId: string");
    expect(app).toContain("nextSnapshot.primaryConversationId");
    expect(app).not.toContain('taskMode === "coding"');
    expect(app).not.toContain("codingConversations");
    expect(app).not.toContain("新しい会話");
    expect(app).not.toContain("最近の会話");
  });

  test("always submits the primary conversation without a workspace", () => {
    const app = source("src/App.tsx");
    expect(app).toContain("{ runId, conversationId, content, workspacePath: null }");
    expect(app).not.toContain("agent-running");
    expect(app).not.toContain("createConversation");
  });

  test("does not expose Codex configuration or coding routing in Settings", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).not.toContain('id: "codex"');
    expect(settings).not.toContain("<CodexSection");
    expect(settings).not.toContain("<h3>coding.assist</h3>");
    expect(settings).not.toContain("getCodexStatus()");
  });

  test("lets users select the conversation reasoning effort in LLM Providers", () => {
    const settings = source("src/features/settings/SettingsPage.tsx");
    expect(settings).toContain('Field label="Reasoning effort"');
    expect(settings).toContain('<option value="low">Low</option>');
    expect(settings).toContain('<option value="medium">Medium (recommended)</option>');
    expect(settings).toContain('<option value="xhigh">Extra high</option>');
  });

  test("renders actual partial and final transcript events", () => {
    const contracts = source("src/lib/contracts.ts");
    const meeting = source("src/features/meeting/useMeetingSession.ts");
    expect(contracts).toContain('type: "transcriptDelta"');
    expect(contracts).toContain('type: "transcriptPartial"');
    expect(meeting).toContain('event.type === "transcriptPartial" || event.type === "transcriptFinal"');
  });
});
