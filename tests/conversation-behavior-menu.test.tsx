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

test("conversation behavior menu keeps diagnosis on its own screen", async () => {
  resetTauriCoreMock();
  const env = installJsdom();
  const root = createRoot(document.getElementById("root")!);
  await i18n.changeLanguage("ja");
  try {
    await act(async () => {
      root.render(
        <ConversationBehaviorMenu
          policy={policy}
          disabled={false}
          onOpenSettings={() => undefined}
          onSetSpeechOutput={() => undefined}
          onSetListeningPace={() => undefined}
          onReset={() => undefined}
        />,
      );
    });
    expect(document.body.textContent).not.toContain("自己診断");
  } finally {
    await act(() => root.unmount());
    env.dom.window.close();
    env.restore();
  }
});
