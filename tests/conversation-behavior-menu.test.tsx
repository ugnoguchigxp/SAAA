import { expect, test } from "bun:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { ConversationBehaviorMenu } from "../src/features/chat/ConversationBehaviorMenu";
import type { ConversationVoicePolicySnapshot } from "../src/lib/contracts";
import i18n from "../src/i18n";
import { resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

const policy: ConversationVoicePolicySnapshot = {
  conversationId: "conversation_primary",
  speechOutput: "inherit",
  listeningPace: "balanced",
  policyRevision: 1,
  updatedAt: "1",
  effectiveSpeechOutput: "speak",
  speechReasonCode: "global_default",
  effectiveListeningPace: "balanced",
  effectiveSilenceTimeoutMs: 1500,
};

test("conversation behavior menu opens diagnosis", async () => {
  resetTauriCoreMock();
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  let opened = 0;
  await i18n.changeLanguage("ja");
  try {
    await act(async () => {
      root.render(
        <ConversationBehaviorMenu
          policy={policy}
          disabled={false}
          onOpenSettings={() => undefined}
          onOpenDiagnosis={() => {
            opened += 1;
          }}
          onSetSpeechOutput={() => undefined}
          onSetListeningPace={() => undefined}
          onReset={() => undefined}
        />,
      );
    });
    const button = [...document.querySelectorAll("button")].find(
      (candidate) => candidate.textContent === "自己診断",
    );
    expect(button).toBeTruthy();
    await act(() => button!.click());
    expect(opened).toBe(1);
  } finally {
    await act(() => root.unmount());
    env.dom.window.close();
    env.restore();
  }
});
