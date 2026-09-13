import { voiceAsrEventOrder } from "./ipcEventOrder";
import { Channel, invoke } from "@tauri-apps/api/core";
import type { StartVoiceAsrSessionInput, VoiceAsrStreamEvent } from "./generated/voiceAsr";
import { guardedReceiver, voiceAsrEventSchema } from "./ipcValidation";
export function createVoiceAsrChannel(
  input: StartVoiceAsrSessionInput,
  onEvent: (event: VoiceAsrStreamEvent) => void,
) {
  const channel = new Channel<unknown>();
  channel.onmessage = guardedReceiver(
    voiceAsrEventSchema,
    "voice-asr",
    onEvent,
    voiceAsrEventOrder(input.sessionId),
    () => {
      void invoke("stop_voice_asr_session", {
        input: { sessionId: input.sessionId, finalizeCurrent: false },
      }).catch(() => undefined);
    },
  );
  return channel;
}
