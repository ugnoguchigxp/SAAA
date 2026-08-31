import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import artifact from "../.s11tnext/catalog.json";
import { createAppCatalog } from "../.s11tnext/catalog.generated";
import { effectiveCaptureSettings } from "../src/features/voice/useAmbientVoiceSession";
import { selectVoicePolicySnapshot } from "../src/features/chat/useConversationVoicePolicy";
import type { ConversationVoicePolicySnapshot, VoiceSettings } from "../src/lib/contracts";

const voiceSettings: VoiceSettings = {
  listeningEnabled: true,
  inputDeviceId: "default",
  outputDeviceId: "default",
  vadSensitivity: "medium",
  silenceTimeoutMs: 1_500,
  allowedLanguages: ["ja"],
  autoSpeak: true,
};

const voicePolicy: ConversationVoicePolicySnapshot = {
  conversationId: "conversation_1",
  speechOutput: "inherit",
  listeningPace: "patient",
  policyRevision: 2,
  updatedAt: "2026-08-31T00:00:00Z",
  effectiveSpeechOutput: "speak",
  speechReasonCode: "global_default",
  effectiveListeningPace: "patient",
  effectiveSilenceTimeoutMs: 2_500,
};

describe("conversation voice behavior", () => {
  test("exposes the tool contract only in the normal Conversation system context", () => {
    const catalog = createAppCatalog(artifact).bind({
      instructionLocale: "en-US",
      trailingNewline: false,
    });
    const conversation = catalog("conversation.respond", {});
    const coding = catalog("codex.read-only", {});

    expect(conversation.content.text).toContain("Use `update_conversation_voice_behavior`");
    expect(conversation.content.text).toContain("Do not treat quotations, translation or writing examples");
    expect(coding.content.text).not.toContain("update_conversation_voice_behavior");
    expect(readFileSync(new URL("../contexts/conversation/respond.context.toml", import.meta.url), "utf8"))
      .toContain("update_conversation_voice_behavior");
  });

  test("maps the conversation listening profile into the next detector snapshot", () => {
    const snapshot = effectiveCaptureSettings(voiceSettings, voicePolicy);

    expect(snapshot?.silenceTimeoutMs).toBe(2_500);
    expect(voiceSettings.silenceTimeoutMs).toBe(1_500);
  });

  test("keeps the global detector timing when no conversation override is loaded", () => {
    expect(effectiveCaptureSettings(voiceSettings, null)).toBe(voiceSettings);
  });

  test("does not let a delayed completion replace a newer manual policy update", () => {
    const stale = { ...voicePolicy, policyRevision: 1, listeningPace: "balanced" as const };

    expect(selectVoicePolicySnapshot(voicePolicy, stale)).toBe(voicePolicy);
  });
});
