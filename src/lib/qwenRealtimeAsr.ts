import { Channel, invoke } from "@tauri-apps/api/core";
import { qwenAsrAudioPayload } from "./qwenAsrAudioPayload";

export type QwenRealtimeAsrEvent =
  | { type: "ready"; sessionId: string }
  | { type: "speechStarted"; sessionId: string; utteranceId: string }
  | { type: "partial"; sessionId: string; utteranceId: string; text: string }
  | { type: "final"; sessionId: string; utteranceId: string; text: string; language: string | null }
  | { type: "failed"; sessionId: string; utteranceId: string | null; message: string }
  | { type: "stopped"; sessionId: string };

export function conversationAsrTransport(): Promise<"http" | "qwen-realtime"> {
  return invoke("conversation_asr_transport");
}

export function startQwenAsrSession(
  sessionId: string,
  onEvent: (event: QwenRealtimeAsrEvent) => void,
  silenceTimeoutMs = 1_500,
): Promise<void> {
  const channel = new Channel<QwenRealtimeAsrEvent>();
  channel.onmessage = onEvent;
  return invoke("start_qwen_asr_session", {
    input: { sessionId, silenceTimeoutMs },
    onEvent: channel,
  });
}

export function appendQwenAsrAudio(
  sessionId: string,
  utteranceId: string,
  audio: Uint8Array,
): Promise<void> {
  return invoke("append_qwen_asr_audio", qwenAsrAudioPayload(sessionId, utteranceId, audio));
}

export function stopQwenAsrSession(sessionId: string): Promise<void> {
  return invoke("stop_qwen_asr_session", { sessionId });
}
