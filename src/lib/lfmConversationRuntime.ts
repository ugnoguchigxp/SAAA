import { Channel, invoke } from "@tauri-apps/api/core";
import type { RuntimeEvent } from "./contracts";

export type LfmUtteranceResult = { handoffId: string | null; requestContent: string | null; speechEpoch: number };

export function receiveLfmUtterance(conversationId: string, utteranceId: string, text: string) {
  const onReceived = new Channel<null>();
  onReceived.onmessage = () => refreshConversation(conversationId);
  return invoke<LfmUtteranceResult>("receive_lfm_utterance", {
    conversationId, utteranceId, text, onReceived,
  }).finally(() => refreshConversation(conversationId));
}

export function speakLfmReply(conversationId: string, utteranceId: string, speechEpoch: number,
  onFailure: (message: string) => void) {
  const onEvent = new Channel<RuntimeEvent>();
  // LFM speech must not suspend ASR or acquire the Qwen turn's frontend speech ownership.
  onEvent.onmessage = (event) => {
    if (event.type === "speechFailed") onFailure(`LFM 音声: ${event.message}`);
  };
  return invoke<void>("speak_lfm_reply", { conversationId, utteranceId, speechEpoch, onEvent });
}

function refreshConversation(conversationId: string) {
  window.dispatchEvent(new window.CustomEvent("saaa:ui-history", { detail: conversationId }));
}

export function isLfmHandoff(sourceId: string | null | undefined): boolean {
  return sourceId?.startsWith("lfm_handoff_") ?? false;
}
