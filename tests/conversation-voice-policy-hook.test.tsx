import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import type { ConversationVoicePolicySnapshot } from "../src/lib/contracts";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

const { useConversationVoicePolicy } = await import("../src/features/chat/useConversationVoicePolicy");

function policyFor(conversationId: string): ConversationVoicePolicySnapshot {
  return {
    conversationId,
    speechOutput: "inherit",
    listeningPace: "balanced",
    policyRevision: 1,
    updatedAt: "1",
    effectiveSpeechOutput: "speak",
    speechReasonCode: "global_default",
    effectiveListeningPace: "balanced",
    effectiveSilenceTimeoutMs: 1_500,
  };
}

function Harness({
  conversationId,
  apiRef,
}: {
  conversationId: string | null;
  apiRef: MutableRefObject<ReturnType<typeof useConversationVoicePolicy> | null>;
}) {
  apiRef.current = useConversationVoicePolicy(conversationId, () => undefined);
  return null;
}

describe("conversation voice policy hook", () => {
  let root: Root | null = null;
  let restore: (() => void) | null = null;
  let policy = policyFor("c1");

  beforeEach(() => {
    policy = policyFor("c1");
    resetTauriCoreMock();
    invokeImpl.handler = async (command, args) => {
      if (command === "get_conversation_voice_policy") {
        return { ...policy, conversationId: (args as { conversationId: string }).conversationId };
      }
      if (command === "update_conversation_voice_policy") {
        const input = (args as { input: { expectedRevision: number; speechOutput: string | null; listeningPace: string | null } }).input;
        policy = {
          ...policy,
          policyRevision: input.expectedRevision + 1,
          speechOutput: input.speechOutput === null ? policy.speechOutput : input.speechOutput as ConversationVoicePolicySnapshot["speechOutput"],
          listeningPace: input.listeningPace === null ? policy.listeningPace : input.listeningPace as ConversationVoicePolicySnapshot["listeningPace"],
        };
        return policy;
      }
      if (command === "reset_conversation_voice_policy") {
        policy = { ...policy, speechOutput: "inherit", listeningPace: "inherit", policyRevision: policy.policyRevision + 1 };
        return policy;
      }
      return command;
    };
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    restore?.();
    restore = null;
  });

  test("loads, updates, and resets the selected conversation policy", async () => {
    restore = installJsdom().restore;
    const { createRoot } = await import("react-dom/client");
    const { createElement } = await import("react");
    const apiRef: MutableRefObject<ReturnType<typeof useConversationVoicePolicy> | null> = { current: null };
    root = createRoot(document.getElementById("root")!);
    await act(async () => root!.render(createElement(Harness, { conversationId: "c1", apiRef })));
    await act(async () => { await Promise.resolve(); });
    expect(apiRef.current!.voicePolicy?.conversationId).toBe("c1");
    await act(async () => { await apiRef.current!.setConversationSpeechOutput("muted"); });
    await act(async () => { await apiRef.current!.setConversationListeningPace("patient"); });
    await act(async () => { await apiRef.current!.resetConversationVoiceOverrides(); });
    expect(policy.speechOutput).toBe("inherit");
    expect(policy.listeningPace).toBe("inherit");
    await act(async () => root!.render(createElement(Harness, { conversationId: null, apiRef })));
    apiRef.current!.clearVoicePolicy();
  });
});
