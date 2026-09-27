import { readFileSync } from "node:fs";
import { join } from "node:path";
const read = (path: string) => readFileSync(join(import.meta.dir, path), "utf8");
export function chatVoiceSource(): string {
  return [
    read("../src/features/voice/VoiceCaptureResources.ts"),
    read("../src/App.tsx"),
    read("../src/features/voice/useAmbientVoiceSession.ts"),
    read("../src/features/voice/ambientVoiceCaptureActions.ts"),
    read("../src/features/voice/ambientVoiceCapture.ts"),
    read("../src/features/voice/ambientNativeVoiceCapture.ts"),
    read("../src/features/voice/ambientWorkletVoiceCapture.ts"),
    read("../src/features/voice/ambientVoiceCaptureContext.ts"),
    read("../src/features/voice/voiceAsrPacketizer.ts"),
    read("../src/features/voice/voiceAsrPacketSender.ts"),
    read("../src/features/voice/voiceSegmentBoundary.ts"),
    read("../src/features/chat/ChatPage.tsx"),
    read("../src/features/chat/useConversationTurn.ts"),
    read("../src/features/chat/conversationTurnControls.ts"),
  ].join("\n");
}
