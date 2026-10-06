import { test, expect } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { routeLocationNotice } from "../src/features/chat/RouteLocationBadge";
import type { RegistryView, PurposeBinding } from "../src/lib/serviceRegistry";

function view(
  larmReachability: RegistryView["larmReachability"],
  fallbackResourceIds: string[],
  cloudAllowed = true,
): RegistryView {
  const binding = (purpose: PurposeBinding["purpose"], resource: string): PurposeBinding => ({
    purpose,
    enabled: true,
    primaryResourceId: resource,
    fallbackResourceIds: purpose === "media.image.generate" ? fallbackResourceIds : [],
    cloudAllowed: purpose === "media.image.generate" ? cloudAllowed : true,
    timeoutMs: 60000,
    review: "ready",
  });
  return {
    snapshot: {
      connections: [
        {
          connectionId: "conn:harness",
          label: "LARM",
          adapterKind: "larm",
          endpoint: "http://192.0.2.1:7001",
          location: "local",
          authentication: "none",
          enabled: true,
        },
        {
          connectionId: "conn:svc-away",
          label: "Replicate",
          adapterKind: "replicate-media",
          endpoint: "https://api.replicate.com/v1",
          location: "cloud",
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
          resourceId: "res:harness-image",
          connectionId: "conn:harness",
          capability: "image-generation",
          model: "",
          enabled: true,
        },
        {
          resourceId: "res:svc-away-image",
          connectionId: "conn:svc-away",
          capability: "image-generation",
          model: "owner/model",
          enabled: true,
        },
      ],
      bindings: [
        binding("conversation.respond", "res:harness-llm"),
        binding("media.image.generate", "res:harness-image"),
      ],
    },
    revision: 1,
    persisted: true,
    larmReachability,
  };
}

test("the chat badge appears only while LARM is unreachable", () => {
  expect(routeLocationNotice(null)).toBeNull();
  expect(routeLocationNotice(view("reachable", []))).toBeNull();
  expect(routeLocationNotice(view("unknown", []))).toBeNull();
  expect(routeLocationNotice(view("unreachable", []))).toContain("利用できる代替先がありません");
  const withFallback = view("unreachable", []);
  withFallback.snapshot.bindings[0].fallbackResourceIds = ["res:svc-away-image"];
  expect(routeLocationNotice(withFallback)).toContain("代替先で応答");
  withFallback.snapshot.bindings[0].cloudAllowed = false;
  expect(routeLocationNotice(withFallback)).toContain("利用できる代替先がありません");
});

test("settings show the LARM state and let LARM purposes pick a cloud fallback", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { PurposeRoutesSection } = await import("../src/features/settings/PurposeRoutesSection");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let current = view("unreachable", ["res:svc-away-image"], false);
  invokeImpl.handler = async (command) => {
    if (command === "get_service_registry") return structuredClone(current);
    throw new Error(command);
  };
  try {
    await act(async () => root.render(<PurposeRoutesSection />));
    const status = document.querySelector('[data-testid="larm-reachability"]')!;
    expect(status.textContent).toContain("接続できません");
    const text = document.body.textContent ?? "";
    expect(text).toContain("LARMに接続できない時（外出時）");
    // A cloud fallback behind a purpose that forbids cloud sending is called out.
    expect(text).toContain("クラウド送信が許可されていません");
    // The configured cloud fallback is listed and removable; the conversation purpose (also on
    // LARM) offers the "add fallback" selector too.
    expect(text).toContain("Replicate / owner/model");
    expect(text).toContain("代替先から外す");
    expect(document.querySelectorAll("select").length).toBeGreaterThan(0);
    expect(text).not.toContain("LARM / ");
    current = view("reachable", []);
    expect(current.larmReachability).toBe("reachable");
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});
