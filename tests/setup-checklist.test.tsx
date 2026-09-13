import { expect, test } from "bun:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import fixtures from "./fixtures/ipc-receivers.json";
import { appSnapshotSchema } from "../src/lib/ipcValidation";
import { nextSetupStep, SetupChecklist } from "../src/features/chat/SetupChecklist";
import { installJsdom } from "./jsdomGlobals";
import i18n from "../src/i18n";

function snapshot() {
  return appSnapshotSchema.parse(structuredClone(fixtures.snapshot));
}
test("setup selects one next operation from persisted settings and route state", () => {
  const value = snapshot();
  value.settings = [];
  expect(nextSetupStep(value)).toBe("provider");
  const configured = snapshot();
  configured.effectiveRoute.state = "failed";
  expect(nextSetupStep(configured)).toBe("connection");
  configured.effectiveRoute.state = "ready";
  const voice = configured.settings.find((setting) => setting.namespace === "voice.runtime")!;
  voice.valueJson.listeningEnabled = false;
  expect(nextSetupStep(configured)).toBe("ready");
  voice.valueJson.listeningEnabled = true;
  configured.voiceProfile.filterEnabled = true;
  configured.voiceProfile.status = "empty";
  expect(nextSetupStep(configured)).toBe("speaker");
  configured.voiceProfile.status = "ready";
  expect(nextSetupStep(configured)).toBe("ready");
  const route = configured.settings.find((setting) => setting.namespace === "routing.tasks")!;
  route.valueJson.voiceTranscribe = { source: "provider", providerId: "missing", timeoutMs: 10000 };
  expect(nextSetupStep(configured)).toBe("asr");
});
test("Japanese and English setup actions reach Settings", async () => {
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  const previousLanguage = i18n.language;
  let opens = 0;
  try {
    for (const [language, label] of [
      ["ja", "設定を開く"],
      ["en", "Open Settings"],
    ]) {
      await act(async () => {
        await i18n.changeLanguage(language);
        root.render(
          <SetupChecklist
            snapshot={snapshot()}
            onOpenSettings={() => {
              opens += 1;
            }}
          />,
        );
      });
      const buttons = document.querySelectorAll("button");
      expect(buttons).toHaveLength(1);
      expect(buttons[0].textContent).toBe(label);
      await act(() => buttons[0].click());
    }
    expect(opens).toBe(2);
  } finally {
    await act(() => root.unmount());
    await i18n.changeLanguage(previousLanguage);
    env.dom.window.close();
    env.restore();
  }
});
