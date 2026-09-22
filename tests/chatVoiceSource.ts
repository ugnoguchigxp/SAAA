import { readFileSync } from "node:fs";
import { join } from "node:path";
export function chatVoiceSource(): string {
  return [
    readFileSync(join(import.meta.dir, "../src/features/voice/VoiceCaptureResources.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/App.tsx"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/useAmbientVoiceSession.ts"), "utf8"),
    readFileSync(
      join(import.meta.dir, "../src/features/voice/ambientVoiceCaptureActions.ts"),
      "utf8",
    ),
    readFileSync(join(import.meta.dir, "../src/features/voice/ambientVoiceCapture.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/voiceAsrPacketizer.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/voiceAsrPacketSender.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/voice/voiceSegmentBoundary.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/chat/ChatPage.tsx"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/chat/useConversationTurn.ts"), "utf8"),
    readFileSync(join(import.meta.dir, "../src/features/chat/conversationTurnControls.ts"), "utf8"),
  ].join("\n");
}
