import { containsSource, readProjectSource as source } from "./sourceContract";
import { describe, expect, test } from "bun:test";

const connections = () =>
  source("src/features/settings/ServiceConnectionsSection.tsx") +
  source("src/features/settings/ConversationTimeoutField.tsx");

describe("settings provider UI contracts", () => {
  test("lets users select the conversation reasoning effort", () => {
    const settings = connections();
    const english = source("src/i18n/locales/enSettings.ts");
    expect(containsSource(settings, 'Field label={t("settings.connection.reasoningEffort")}')).toBe(
      true,
    );
    expect(containsSource(settings, 't("settings.connection.low")')).toBe(true);
    expect(containsSource(settings, 't("settings.connection.medium")')).toBe(true);
    expect(containsSource(settings, 't("settings.connection.extraHigh")')).toBe(true);
    expect(containsSource(english, 'reasoningEffort: "Reasoning effort (LLM)"')).toBe(true);
    expect(containsSource(settings, 'Field label="Maximum output tokens"')).toBe(false);
  });

  test("lets users configure the LLM timeout in seconds", () => {
    const settings = connections();
    const japanese = source("src/i18n/locales/jaSettings.ts");
    expect(containsSource(settings, "<ConversationTimeoutField")).toBe(true);
    expect(containsSource(settings, "conversationTimeoutMsFromSecondsInput(next)")).toBe(true);
    expect(containsSource(settings, "aria-invalid={fieldInvalid}")).toBe(true);
    expect(containsSource(settings, "onValidityChange(!fieldInvalid)")).toBe(true);
    expect(containsSource(settings, 'resolution?.revision === "agent-connection.v1"')).toBe(true);
    expect(containsSource(settings, "t(invalid")).toBe(true);
    expect(containsSource(japanese, 'llmTimeoutSeconds: "LLMタイムアウト（秒）"')).toBe(true);
  });

  test("configures Agent Connection and limits LAN discovery to compatible addresses", () => {
    const settings = connections();
    const japanese = source("src/i18n/locales/jaSettings.ts");
    const runtime = [
      source("src/lib/providerRuntime.ts"),
      source("src/lib/localProviderAddress.ts"),
    ].join("\n");
    const dynamicLan = [
      source("src-tauri/src/providers/dynamic_lan/mod.rs"),
      source("src-tauri/src/providers/dynamic_lan/http.rs"),
      source("src-tauri/src/providers/dynamic_lan/credential.rs"),
      source("src-tauri/src/providers/dynamic_lan/probe.rs"),
      source("src-tauri/src/providers/dynamic_lan/mod.d/02.rs"),
      source("src-tauri/src/providers/dynamic_lan/urls.rs"),
      source("src-tauri/src/providers/dynamic_lan/validate.rs"),
    ].join("\n");
    expect(containsSource(settings, 'Field label={t("settings.connection.harnessAddress")}')).toBe(
      true,
    );
    expect(containsSource(settings, 't("settings.connection.description")')).toBe(true);
    expect(containsSource(japanese, "control APIにはLARM_API_TOKENが必須")).toBe(true);
    expect(containsSource(japanese, "接続を確認")).toBe(true);
    expect(containsSource(japanese, "Agent Connectionのclaim・LLMヘルスチェックに成功")).toBe(true);
    expect(containsSource(settings, 'next.revision === "agent-connection.v1"')).toBe(true);
    expect(containsSource(settings, "legacyDynamicLanHost(address)")).toBe(true);
    expect(containsSource(runtime, "new URL(address)")).toBe(true);
    expect(containsSource(runtime, 'url.protocol === "http:"')).toBe(true);
    expect(containsSource(runtime, 'url.port === "9810"')).toBe(true);
    expect(containsSource(settings, "harness: { ...providers.harness, address }")).toBe(true);
    expect(containsSource(settings, 'placeholder="http://provider.local:9810"')).toBe(true);
    expect(containsSource(dynamicLan, 'format!("http://{host}:{CONTROL_PORT}/")')).toBe(true);
    expect(containsSource(dynamicLan, 'Command::new("ssh")')).toBe(false);
    expect(containsSource(dynamicLan, '.join("v3/agent-profiles")')).toBe(true);
    expect(containsSource(dynamicLan, '.extend(["v1", "agent-connections", id])')).toBe(true);
    expect(containsSource(dynamicLan, '.push("claim")')).toBe(true);
    expect(containsSource(dynamicLan, '"openai-provider-v1"')).toBe(true);
    expect(containsSource(dynamicLan, "endpoint: descriptor.configuration.fields.base_url")).toBe(
      true,
    );
    expect(containsSource(dynamicLan, 'value == "openai.chat-completions.v1"')).toBe(true);
    expect(containsSource(dynamicLan, 'CredentialLoadError::new("credential_missing")')).toBe(true);
    expect(containsSource(dynamicLan, 'CredentialLoadError::new("credential_conflict")')).toBe(
      true,
    );
  });
});
