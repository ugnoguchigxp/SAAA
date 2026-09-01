import { describe, expect, test } from "bun:test";
import { defaultSettingsDraft } from "../src/features/settings/settingsDefaults";
import { credentialCleanupProviderIds } from "../src/features/settings/settingsDraft";
import { legacyDynamicLanHost } from "../src/lib/providerRuntime";
import { modelProvidersSettingsSchema } from "../src/lib/providerSchemas";

describe("reviewed settings boundaries", () => {
  test("only the original Dynamic LAN address shape enables legacy fallback", () => {
    expect(legacyDynamicLanHost("http://provider.local:9810")).toBe("provider.local");
    expect(legacyDynamicLanHost("https://provider.example")).toBeNull();
    expect(legacyDynamicLanHost("http://provider.local:9811")).toBeNull();
    expect(legacyDynamicLanHost("http://provider.local:9810/harness")).toBeNull();
    expect(legacyDynamicLanHost("http://example.com:9810")).toBeNull();
    expect(legacyDynamicLanHost("http://[::1]:9810")).toBeNull();
  });

  test("cleans credentials after removal or switching authentication off", () => {
    const source = structuredClone(defaultSettingsDraft);
    source.providers.providers.push({
      kind: "openai-compatible",
      id: "cloud-llm",
      enabled: true,
      label: "Cloud LLM",
      location: "cloud",
      endpoint: "https://example.com/v1",
      model: "model",
      authentication: "api-key",
    });
    const removed = structuredClone(source);
    removed.providers.providers = removed.providers.providers.filter(({ id }) => id !== "cloud-llm");
    expect(credentialCleanupProviderIds(source, removed)).toEqual(["cloud-llm"]);

    const authenticationOff = structuredClone(source);
    const provider = authenticationOff.providers.providers.find(({ id }) => id === "cloud-llm");
    if (provider && "authentication" in provider) provider.authentication = "none";
    expect(credentialCleanupProviderIds(source, authenticationOff)).toEqual(["cloud-llm"]);
    expect(credentialCleanupProviderIds(source, source)).toEqual([]);

    const alreadyOff = structuredClone(authenticationOff);
    expect(credentialCleanupProviderIds(alreadyOff, alreadyOff)).toEqual(["cloud-llm"]);
  });

  test("accepts local provider names and rejects silently trimmed metadata", () => {
    const settings = structuredClone(defaultSettingsDraft.providers);
    settings.providers.push({
      kind: "openai-compatible",
      id: "local-llm",
      enabled: true,
      label: "Local LLM",
      location: "local",
      endpoint: "http://llm.local:8080/v1",
      model: "model",
      authentication: "none",
    });
    expect(() => modelProvidersSettingsSchema.parse(settings)).not.toThrow();
    const provider = settings.providers.at(-1);
    if (provider?.kind === "openai-compatible") provider.model = " model";
    expect(() => modelProvidersSettingsSchema.parse(settings)).toThrow("surrounding whitespace");
  });

  test("requires ASR providers to auto-detect before applying the language allowlist", () => {
    const settings = structuredClone(defaultSettingsDraft.providers);
    const asr = {
      kind: "cloud-asr",
      id: "cloud-asr",
      enabled: true,
      label: "Cloud ASR",
      location: "cloud",
      endpoint: "https://api.example.com/v1",
      model: "asr-model",
      language: "auto",
      authentication: "none",
    };
    settings.providers.push(asr as typeof settings.providers[number]);
    expect(() => modelProvidersSettingsSchema.parse(settings)).not.toThrow();
    asr.language = "ja";
    expect(() => modelProvidersSettingsSchema.parse(settings)).toThrow();
  });
});
