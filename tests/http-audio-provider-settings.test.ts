import { describe, expect, test } from "bun:test";
import { defaultSettingsDraft } from "../src/features/settings/settingsDefaults";
import { modelProvidersSettingsSchema } from "../src/lib/providerSchemas";

describe("HTTP audio provider settings", () => {
  test("keeps existing ASR providers on HTTP and accepts an explicit Qwen realtime provider", () => {
    const settings = structuredClone(defaultSettingsDraft.providers);
    const base = {
      kind: "cloud-asr" as const,
      id: "qwen-asr",
      enabled: true,
      label: "Qwen ASR",
      location: "cloud" as const,
      endpoint: "https://dashscope-intl.aliyuncs.com/api-ws/v1/realtime",
      model: "qwen3-asr-flash-realtime",
      language: "auto" as const,
      authentication: "api-key" as const,
    };
    const old = modelProvidersSettingsSchema.parse({ ...settings, providers: [base] });
    expect(old.providers[0]?.kind === "cloud-asr" && old.providers[0].transport).toBeUndefined();
    expect(() =>
      modelProvidersSettingsSchema.parse({
        ...settings,
        providers: [{ ...base, transport: "qwen-realtime" }],
      }),
    ).not.toThrow();
    expect(() =>
      modelProvidersSettingsSchema.parse({
        ...settings,
        providers: [{ ...base, transport: "qwen-realtime", authentication: "none" }],
      }),
    ).toThrow();
  });

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
