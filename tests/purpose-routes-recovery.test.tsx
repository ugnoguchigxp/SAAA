import { test, expect } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import type { RegistryView, RegistrySnapshot } from "../src/lib/serviceRegistry";

test("a failed key save resumes the same persisted draft after reload", async () => {
  const environment = installJsdom();
  resetTauriCoreMock();
  const { PurposeRoutesSection } = await import("../src/features/settings/PurposeRoutesSection");
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  let current: RegistryView = {
    snapshot: { connections: [], resources: [], bindings: [] },
    revision: 0,
    persisted: false,
  };
  let keyStored = false;
  let rejectKey = true;
  const targetIds: string[] = [];
  invokeImpl.handler = async (command, args) => {
    if (command === "get_service_registry") return structuredClone(current);
    if (command === "save_service_registry") {
      const input = args as { snapshot: RegistrySnapshot; expectedRevision: number };
      if (input.expectedRevision !== current.revision) throw new Error("revision conflict");
      current = {
        snapshot: structuredClone(input.snapshot),
        revision: current.revision + 1,
        persisted: true,
      };
      return structuredClone(current);
    }
    if (command === "set_service_connection_secret") {
      const input = args as { connectionId: string; apiKey: string };
      targetIds.push(input.connectionId);
      if (rejectKey) throw new Error("key storage failed");
      expect(input.apiKey).toBe("fixture-key");
      keyStored = true;
      return { state: "configured" };
    }
    if (command === "get_service_connection_secret_state")
      return { state: keyStored ? "configured" : "missing" };
    throw new Error(command);
  };
  const button = (label: string) =>
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => b.textContent === label,
    )!;
  const fill = async (input: HTMLInputElement, value: string) =>
    act(async () => {
      Object.getOwnPropertyDescriptor(
        environment.dom.window.HTMLInputElement.prototype,
        "value",
      )!.set!.call(input, value);
      input.dispatchEvent(new environment.dom.window.Event("input", { bubbles: true }));
    });
  try {
    await act(async () => root.render(<PurposeRoutesSection />));
    const inputs = [...document.querySelectorAll<HTMLInputElement>("input")];
    for (const [i, value] of ["Cloud", "https://example.invalid/v1", "m", "fixture-key"].entries())
      await fill(inputs[i], value);
    await act(async () => button("登録").click());
    expect(current.snapshot.connections).toHaveLength(1);
    expect(current.snapshot.connections[0].enabled).toBe(false);
    expect(document.querySelector('[role="status"]')?.textContent).toContain("key storage failed");
    await act(async () => button("再読込").click());
    rejectKey = false;
    const resumeKey = document.querySelector<HTMLInputElement>(
      '[aria-label="登録したサービス"] input',
    )!;
    await fill(resumeKey, "fixture-key");
    await act(async () => button("登録を再開・有効にする").click());
    expect(current.snapshot.connections).toHaveLength(1);
    expect(current.snapshot.connections[0].enabled).toBe(true);
    expect(targetIds).toEqual(["conn:svc-cloud", "conn:svc-cloud"]);
    expect(JSON.stringify(current.snapshot)).not.toContain("fixture-key");
    await act(async () => button("無効にする").click());
    expect(current.snapshot.connections[0].enabled).toBe(false);
    await act(async () => button("削除").click());
    expect(current.snapshot.connections).toHaveLength(0);
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    resetTauriCoreMock();
  }
});
