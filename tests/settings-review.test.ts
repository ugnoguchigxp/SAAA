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
    removed.providers.providers = removed.providers.providers.filter(
      ({ id }) => id !== "cloud-llm",
    );
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

  test("accepts an Agent Session LLM with explicit discovery and session paths", () => {
    const settings = structuredClone(defaultSettingsDraft.providers);
    settings.providers.push({
      kind: "agent-session",
      id: "muse-agent",
      enabled: true,
      label: "Muse Agent",
      location: "local",
      baseUrl: "http://127.0.0.1:44449",
      model: "muse/muse-spark-1.3-contributor",
      modelsPath: "/v1/agents/models?runtime=muse",
      sessionsPath: "/v1/agents/sessions",
      authentication: "none",
    });
    expect(() => modelProvidersSettingsSchema.parse(settings)).not.toThrow();
    const provider = settings.providers.at(-1);
    if (provider?.kind !== "agent-session") throw new Error("Agent Session fixture missing");
    provider.modelsPath = "//other.example/v1/agents/models?runtime=muse";
    expect(() => modelProvidersSettingsSchema.parse(settings)).toThrow();
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
    settings.providers.push(asr as (typeof settings.providers)[number]);
    expect(() => modelProvidersSettingsSchema.parse(settings)).not.toThrow();
    asr.language = "ja";
    expect(() => modelProvidersSettingsSchema.parse(settings)).toThrow();
  });
});

describe("HTTP audio provider settings", () => {
  test("reads existing WAV settings and accepts local PCM without WS fields", () => {
    for (const [kind, extra] of [
      ["cloud-asr", { language: "auto" }],
      ["cloud-tts", { voice: "local-voice" }],
    ] as const) {
      const settings = structuredClone(defaultSettingsDraft.providers);
      const provider = {
        kind,
        id: `http-${kind}`,
        enabled: true,
        label: "Local HTTP",
        location: "local",
        endpoint: "http://127.0.0.1:9000/proxy/v1",
        model: "local-model",
        authentication: "none",
        ...extra,
      };
      const parsed = modelProvidersSettingsSchema.parse({ ...settings, providers: [provider] });
      if (parsed.providers[0]?.kind === "cloud-tts")
        expect(parsed.providers[0].responseFormat).toBe("wav");
      if (kind === "cloud-tts") {
        expect(() =>
          modelProvidersSettingsSchema.parse({
            ...settings,
            providers: [{ ...provider, responseFormat: "pcm" }],
          }),
        ).not.toThrow();
        expect(() =>
          modelProvidersSettingsSchema.parse({
            ...settings,
            providers: [{ ...provider, responseFormat: "mp3" }],
          }),
        ).toThrow();
      }
      expect(() =>
        modelProvidersSettingsSchema.parse({
          ...settings,
          providers: [{ ...provider, endpoint: "http://public.example/v1" }],
        }),
      ).toThrow();
      expect(() =>
        modelProvidersSettingsSchema.parse({
          ...settings,
          providers: [{ ...provider, endpoint: "http://token@localhost/v1" }],
        }),
      ).toThrow();
    }
  });
});
