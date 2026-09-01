import { describe, expect, test } from "bun:test";
import { isMeetingBlocking, toMessage } from "../src/lib/appHelpers";
import { findPrimaryRoute, updateConversationTimestamp } from "../src/lib/conversationRouting";
import { mergePcmFrames } from "../src/lib/pcm";
import type { AppSnapshot, SettingsDocument } from "../src/lib/contracts";

const emptySnapshot = (): AppSnapshot => ({
  settings: [],
  conversations: [],
  primaryConversationId: "",
  effectiveRoute: { providerId: null, label: "モデル未選択", location: null, state: "unchecked", fallbackUsed: false, reasonCode: "test", updatedAt: null },
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
});

describe("chat helpers", () => {
  test("mergePcmFrames concatenates frames in order", () => {
    const merged = mergePcmFrames([new Float32Array([1, 2]), new Float32Array([3])], 3);
    expect(Array.from(merged)).toEqual([1, 2, 3]);
  });

  test("findPrimaryRoute reads the conversation route and falls back to dynamic_lan", () => {
    expect(findPrimaryRoute([])).toBe("lan-llm-dynamic");
    const documents: SettingsDocument[] = [{
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 14,
      valueJson: { conversationRespond: { primaryProviderId: "local-openai-compatible" } },
      updatedAt: "1",
    }];
    expect(findPrimaryRoute(documents)).toBe("local-openai-compatible");
  });

  test("updateConversationTimestamp promotes the active conversation and truncates title", () => {
    const snapshot = emptySnapshot();
    snapshot.conversations = [
      { id: "other", title: "Other", taskMode: "conversation", createdAt: "1", updatedAt: "1" },
      { id: "active", title: null, taskMode: "conversation", createdAt: "1", updatedAt: "1" },
    ];
    const updated = updateConversationTimestamp(snapshot, "active", "a".repeat(80));
    expect(updated.conversations[0]?.id).toBe("active");
    expect(updated.conversations[0]?.title).toBe("a".repeat(60));
    expect(updated.conversations[0]?.updatedAt).toBe("pending");
    expect(updateConversationTimestamp(snapshot, "missing", "x")).toBe(snapshot);
  });

  test("isMeetingBlocking covers preflight through stopping", () => {
    expect(isMeetingBlocking("idle")).toBe(false);
    expect(isMeetingBlocking("preflight")).toBe(true);
    expect(isMeetingBlocking("active")).toBe(true);
    expect(isMeetingBlocking("paused")).toBe(true);
    expect(isMeetingBlocking("stopping")).toBe(true);
    expect(isMeetingBlocking("review")).toBe(false);
  });

  test("toMessage unwraps Error and stringifies other values", () => {
    expect(toMessage(new Error("boom"))).toBe("boom");
    expect(toMessage("plain")).toBe("plain");
  });
});
