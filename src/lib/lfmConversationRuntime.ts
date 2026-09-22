import { Channel, invoke } from "@tauri-apps/api/core";
import type { RuntimeEvent } from "./contracts";

export type LfmUtteranceResult = {
  reasoningRequestId: string | null;
  requestContent: string | null;
  speechEpoch: number;
  ignoredAsSelfSpeech: boolean;
  hasReply: boolean;
};

export function receiveLfmUtterance(conversationId: string, utteranceId: string, text: string) {
  const onReceived = new Channel<null>();
  onReceived.onmessage = () => refreshConversation(conversationId);
  return invoke<LfmUtteranceResult>("receive_lfm_utterance", {
    conversationId,
    utteranceId,
    text,
    onReceived,
  }).finally(() => refreshConversation(conversationId));
}

export function speakLfmReply(
  conversationId: string,
  utteranceId: string,
  speechEpoch: number,
  onEventMessage: (event: RuntimeEvent) => void,
) {
  const onEvent = new Channel<RuntimeEvent>();
  // Playback is native audio, outside the WebView, so browser echoCancellation has no reference.
  // The session pauses capture for this playback without taking the Qwen speech-run slot.
  onEvent.onmessage = onEventMessage;
  return invoke<void>("speak_lfm_reply", { conversationId, utteranceId, speechEpoch, onEvent });
}

function refreshConversation(conversationId: string) {
  window.dispatchEvent(new window.CustomEvent("saaa:ui-history", { detail: conversationId }));
}

export function isLfmReasoningRequest(sourceId: string | null | undefined): boolean {
  return sourceId?.startsWith("lfm_reasoning_") || sourceId?.startsWith("lfm_handoff_") || false;
}
