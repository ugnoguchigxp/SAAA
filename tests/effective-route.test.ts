import { describe, expect, test } from "bun:test";
import {
  resolveModelProviderStatus,
  updateEffectiveRoute,
} from "../src/lib/conversationRouting";
import type { AppSnapshot } from "../src/lib/contracts";

function snapshot(): AppSnapshot {
  return {
    settings: [],
    conversations: [],
    primaryConversationId: "",
    effectiveRoute: {
      providerId: null,
      label: "モデル未選択",
      location: null,
      state: "unchecked",
      fallbackUsed: false,
      reasonCode: "test",
      updatedAt: null,
    },
    larmRuntime: { state: "disabled", message: "", contractCommit: "unknown" },
    voiceProfile: {
      status: "empty",
      filterEnabled: false,
      runtimeAvailable: false,
      runtimeMessage: "",
      sampleCount: 0,
      targetSampleCount: 5,
      totalDurationMs: 0,
      minimumDurationMs: 20_000,
      threshold: 0.55,
      samples: [],
    },
  };
}

describe("effective provider route", () => {
  test("readiness comes from the runtime snapshot, not configured endpoint strings", () => {
    const current = snapshot();
    current.effectiveRoute = {
      providerId: "local",
      label: "Local",
      location: "local",
      state: "unchecked",
      fallbackUsed: false,
      reasonCode: "not-probed",
      updatedAt: null,
    };
    expect(resolveModelProviderStatus(current).ready).toBe(false);
    current.effectiveRoute.state = "ready";
    expect(resolveModelProviderStatus(current).ready).toBe(true);
  });

  test("runtime provider events expose the selected fallback route", () => {
    const current = snapshot();
    current.settings = [
      {
        namespace: "providers.model",
        key: "default",
        schemaVersion: 12,
        valueJson: {
          harness: { address: "http://localhost:9810" },
          providers: [
            {
              kind: "openai-compatible",
              id: "primary",
              enabled: true,
              label: "Primary",
              location: "cloud",
              endpoint: "https://example.test/v1",
              model: "primary",
              authentication: "api-key",
            },
            {
              kind: "openai-compatible",
              id: "fallback",
              enabled: true,
              label: "Fallback",
              location: "local",
              endpoint: "http://localhost:11434/v1",
              model: "fallback",
              authentication: "none",
            },
          ],
          reasoningEffort: "medium",
        },
        updatedAt: "1",
      },
      {
        namespace: "routing.tasks",
        key: "default",
        schemaVersion: 12,
        valueJson: {
          conversationRespond: {
            source: "provider",
            primaryProviderId: "primary",
            fallbackProviderIds: ["fallback"],
            timeoutMs: 60_000,
          },
          voiceTranscribe: {
            source: "harness",
            providerId: null,
            timeoutMs: 120_000,
          },
          voiceSpeak: {
            source: "provider",
            providerId: "system-tts",
            timeoutMs: 30_000,
          },
          codingAssist: {
            providerId: "codex-sdk",
            timeoutMs: 60_000,
            readOnly: true,
            networkEnabled: false,
            webSearchEnabled: false,
          },
        },
        updatedAt: "1",
      },
    ];
    const updated = updateEffectiveRoute(current, "fallback", "active", {
      reasonCode: "turn-active",
    });
    expect(updated.effectiveRoute).toMatchObject({
      label: "Fallback",
      location: "local",
      fallbackUsed: true,
      reasonCode: "fallback-route",
    });
  });
});
