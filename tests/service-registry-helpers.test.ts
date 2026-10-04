import { describe, expect, test } from "bun:test";
import {
  addChatService,
  assignPrimary,
  candidatesFor,
  purposesUsing,
  setConnectionEnabled,
  type RegistrySnapshot,
} from "../src/lib/serviceRegistry";

const base: RegistrySnapshot = {
  connections: [
    {
      connectionId: "conn:harness",
      label: "LARM",
      adapterKind: "larm",
      endpoint: "http://larm.test",
      location: "local",
      authentication: "none",
      enabled: true,
    },
  ],
  resources: [
    {
      resourceId: "res:harness-llm",
      connectionId: "conn:harness",
      capability: "text-generation",
      model: "",
      enabled: true,
    },
    {
      resourceId: "res:harness-asr",
      connectionId: "conn:harness",
      capability: "transcription",
      model: "",
      enabled: true,
    },
  ],
  bindings: [
    {
      purpose: "conversation.respond",
      enabled: true,
      primaryResourceId: "res:harness-llm",
      fallbackResourceIds: [],
      timeoutMs: 60000,
      storedPrimaryResourceId: "res:legacy",
      review: "needs-review",
    },
  ],
};

describe("service registry helpers", () => {
  test("a new service is a disabled draft with a credential reference and no key", () => {
    const { snapshot, connectionId, resourceId } = addChatService(base, {
      label: "My Cloud",
      endpoint: "https://api.example.test/v1/",
      model: "m",
      authentication: "api-key",
      location: "cloud",
    });
    const connection = snapshot.connections.find(
      (item) => item.connectionId === connectionId,
    )!;
    expect(connection.enabled).toBe(false);
    expect(connection.endpoint).toBe("https://api.example.test/v1");
    expect(connection.credentialRef?.account).toBe(connectionId);
    expect(JSON.stringify(snapshot).toLowerCase()).not.toContain("apikey");
    expect(
      snapshot.resources.find((item) => item.resourceId === resourceId)
        ?.enabled,
    ).toBe(false);
    expect(
      addChatService(snapshot, {
        label: "My Cloud",
        endpoint: "https://x",
        model: "m",
        authentication: "none",
        location: "cloud",
      }).connectionId,
    ).not.toBe(connectionId);
  });

  test("choosing a resource applies it and clears the review state", () => {
    const { snapshot, resourceId, connectionId } = addChatService(base, {
      label: "Cloud",
      endpoint: "https://api.example.test/v1",
      model: "m",
      authentication: "none",
      location: "cloud",
    });
    const enabled = setConnectionEnabled(snapshot, connectionId, true);
    const applied = assignPrimary(enabled, "conversation.respond", resourceId);
    const binding = applied.bindings[0];
    expect(binding.review).toBe("ready");
    expect(binding.primaryResourceId).toBe(resourceId);
    expect(binding.storedPrimaryResourceId).toBeUndefined();
    expect(purposesUsing(applied, connectionId)).toEqual([
      "conversation.respond",
    ]);
  });

  test("candidates only list resources with the purpose capability", () => {
    expect(
      candidatesFor(base, "voice.transcribe").map(
        (item) => item.resource.resourceId,
      ),
    ).toEqual(["res:harness-asr"]);
    expect(candidatesFor(base, "voice.speak")).toEqual([]);
  });

  test("clearing a purpose disables it without choosing a replacement", () => {
    const cleared = assignPrimary(base, "conversation.respond", null);
    expect(cleared.bindings[0].enabled).toBe(false);
    expect(cleared.bindings[0].primaryResourceId).toBeNull();
  });
});
