import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

function source(path: string): string {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}

describe("settings provider UI contracts", () => {
  test("lets users select the conversation reasoning effort", () => {
    const settings = source("src/features/settings/ServiceConnectionsSection.tsx");
    const english = source("src/i18n/locales/en.ts");
    expect(settings).toContain('Field label={t("settings.connection.reasoningEffort")}');
    expect(settings).toContain('t("settings.connection.low")');
    expect(settings).toContain('t("settings.connection.medium")');
    expect(settings).toContain('t("settings.connection.extraHigh")');
    expect(english).toContain('reasoningEffort: "Reasoning effort (LLM)"');
    expect(settings).not.toContain('Field label="Maximum output tokens"');
  });

  test("lets users configure the LLM timeout in seconds", () => {
    const settings = source("src/features/settings/ServiceConnectionsSection.tsx");
    const japanese = source("src/i18n/locales/ja.ts");
    expect(settings).toContain("<ConversationTimeoutField");
    expect(settings).toContain("conversationTimeoutMsFromSecondsInput(next)");
    expect(settings).toContain('aria-invalid={invalid}');
    expect(japanese).toContain('llmTimeoutSeconds: "LLMタイムアウト（秒）"');
  });

  test("configures the Harness address and limits legacy dynamic discovery to compatible addresses", () => {
    const settings = source("src/features/settings/ServiceConnectionsSection.tsx");
    const japanese = source("src/i18n/locales/ja.ts");
    const runtime = source("src/lib/providerRuntime.ts");
    const dynamicLan = [
      source("src-tauri/src/providers/dynamic_lan/mod.rs"),
      source("src-tauri/src/providers/dynamic_lan/http.rs"),
      source("src-tauri/src/providers/dynamic_lan/validate.rs"),
    ].join("\n");
    expect(settings).toContain('Field label={t("settings.connection.harnessAddress")}');
    expect(settings).toContain('t("settings.connection.description")');
    expect(japanese).toContain("一つのアドレスからLLM・ASR・TTSを個別に解決");
    expect(settings).toContain("legacyDynamicLanHost(address)");
    expect(runtime).toContain("new URL(address)");
    expect(runtime).toContain('url.protocol === "http:"');
    expect(runtime).toContain('url.port === "9810"');
    expect(settings).toContain("harness: { address }");
    expect(settings).toContain('placeholder="http://provider.local:9810"');
    expect(dynamicLan).toContain('format!("http://{host}:{CONTROL_PORT}/")');
    expect(dynamicLan).not.toContain('Command::new("ssh")');
    expect(dynamicLan).toContain('.join("v1/agent-profiles")');
    expect(dynamicLan).toContain('.extend(["v1", "agent-connections", id])');
    expect(dynamicLan).toContain('.push("claim")');
    expect(dynamicLan).toContain('"openai-provider-v1"');
  });
});
