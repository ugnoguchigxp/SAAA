import { describe, expect, test } from "bun:test";
import { defaultSettingsDraft } from "../src/features/settings/settingsDefaults";
import {
  documentsFromDraft,
  reconcileSavedDraft,
} from "../src/features/settings/settingsDraft";
import { modelProvidersSettingsSchema } from "../src/lib/providerSchemas";
import { validateSettingsDocuments } from "../src/lib/schemas";
import providerCases from "./fixtures/provider-validation.json";

describe("settings regressions", () => {
  test("matches the shared provider endpoint contract", () => {
    for (const fixture of providerCases) {
      const settings = structuredClone(defaultSettingsDraft.providers);
      settings.providers = [{
        kind: "openai-compatible", id: "fixture", enabled: true, label: "Fixture",
        location: fixture.location as "local" | "cloud", endpoint: fixture.endpoint,
        model: "model", authentication: "none",
      }];
      expect(modelProvidersSettingsSchema.safeParse(settings).success, fixture.name).toBe(fixture.valid);
    }
  });
  test("malformed URLs are validation failures rather than thrown TypeErrors", () => {
    const malformed = structuredClone(defaultSettingsDraft.providers);
    malformed.providers.push({
      kind: "openai-compatible", id: "broken", enabled: true, label: "Broken",
      location: "local", endpoint: "http://[", model: "model", authentication: "none",
    });
    expect(() => modelProvidersSettingsSchema.safeParse(malformed)).not.toThrow();
    expect(modelProvidersSettingsSchema.safeParse(malformed).success).toBe(false);
  });

  test("rejects a non-LLM fallback provider", () => {
    const draft = structuredClone(defaultSettingsDraft);
    const tts = draft.providers.providers.find((provider) => provider.kind === "system-tts");
    if (!tts) throw new Error("fixture TTS provider missing");
    tts.enabled = true;
    draft.routing.conversationRespond.source = "provider";
    draft.routing.conversationRespond.primaryProviderId = "lan-llm-dynamic";
    draft.routing.conversationRespond.timeoutMs = 120_000;
    draft.routing.conversationRespond.fallbackProviderIds = [tts.id];
    expect(() => validateSettingsDocuments(documentsFromDraft(draft))).toThrow(
      "Fallback provider does not support LLM",
    );
  });

  test("applies normalized saves only when the submitted draft is still current", () => {
    const submitted = structuredClone(defaultSettingsDraft);
    submitted.codex.agentName = " Renamed ";
    const fingerprint = JSON.stringify(submitted);
    const saved = documentsFromDraft(submitted).map((document) => ({
      ...document,
      updatedAt: "2026-09-06T00:00:00Z",
    }));
    expect(reconcileSavedDraft(submitted, fingerprint, saved).codex.agentName).toBe("Renamed");

    const editedWhileSaving = structuredClone(submitted);
    editedWhileSaving.codex.agentName = "New edit";
    expect(reconcileSavedDraft(editedWhileSaving, fingerprint, saved).codex.agentName).toBe("New edit");
  });
});
