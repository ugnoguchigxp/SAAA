import type { MutableRefObject } from "react";
import { toMessage } from "../../lib/appHelpers";
import type { ConversationSession, PendingConversationPrompt, SubmitPromptOptions } from "../../lib/conversationSession";
import { transcribeAudio } from "../../lib/runtime";
import type { VoiceSession, VoiceSessionEvent } from "../../lib/voiceSession";
import type { VoiceSegmentQueue } from "./voiceSegmentQueue";

export async function drainVoiceSegmentQueue(context: {
  queue: VoiceSegmentQueue;
  session: MutableRefObject<VoiceSession>;
  disposed: MutableRefObject<boolean>;
  conversation: MutableRefObject<ConversationSession>;
  pendingPrompts: MutableRefObject<PendingConversationPrompt[]>;
  applyEvent: (event: VoiceSessionEvent) => unknown;
  setTranscript: (transcript: string) => void;
  setError: (message: string) => void;
  setRuntimeActivity: (update: (current: string[]) => string[]) => void;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
}): Promise<void> {
  if (context.session.current.processingSegments) return;
  context.applyEvent({ type: "processingStarted" });
  try {
    while (context.queue.length > 0) {
      const segment = context.queue.shift();
      if (!segment) break;
      const runId = `voice_${crypto.randomUUID()}`;
      context.applyEvent({ type: "transcriptionStarted", runId });
      try {
        const transcript = await transcribeAudio({
          runId,
          conversationId: segment.conversationId,
          samples: segment.samples,
          sampleRate: segment.sampleRate,
        }, (event) => {
          if (context.disposed.current || context.session.current.cancellationRequested || event.runId !== runId) return;
          if (event.type === "transcriptFinal") context.setTranscript(event.text);
        });
        clearSamples(segment);
        if (context.session.current.cancellationRequested || !transcript.trim()) continue;
        if (context.disposed.current) continue;
        context.setTranscript(transcript);
        if (context.conversation.current.speechRunId) await context.stopSpeech();
        if (context.disposed.current) continue;
        if (context.conversation.current.runId) {
          if (context.pendingPrompts.current.length >= 2) {
            context.setError("応答待ちの音声クエリーが上限に達したため、新しい発話は送信しませんでした。");
          } else {
            context.pendingPrompts.current.push({ content: transcript, inputOrigin: "voice" });
            context.setRuntimeActivity((current) => [...current, "Voice query queued until the active response completes"].slice(-8));
          }
        } else {
          void context.submitPrompt(transcript, { inputOrigin: "voice" });
        }
      } catch (cause) {
        clearSamples(segment);
        const message = toMessage(cause);
        if (!context.session.current.cancellationRequested
          && !(segment.ttsActiveAtCapture && message.startsWith("TARGET_SPEAKER_REJECTED"))
          && !context.disposed.current) {
          context.setError(voiceSegmentError(message));
        }
      } finally {
        context.applyEvent({ type: "transcriptionFinished", runId });
      }
    }
  } finally {
    context.applyEvent({ type: "processingFinished" });
  }
}

function clearSamples(segment: { samples: Float32Array }): void {
  segment.samples.fill(0);
  segment.samples = new Float32Array();
}

function voiceSegmentError(message: string): string {
  if (message.startsWith("TARGET_SPEAKER_REJECTED")) return "登録した本人の声として確認できなかったため、文字起こしへ送信しませんでした。";
  if (message.startsWith("ASR_LANGUAGE_NOT_ALLOWED")) return "登録されていない言語だったため、文字起こしを会話へ送信しませんでした。";
  if (message.startsWith("ASR_LANGUAGE_UNKNOWN")) return "使用言語を判定できなかったため、文字起こしを会話へ送信しませんでした。";
  if (message.startsWith("ASR_NO_SPEECH")) return "発話を確認できなかったため、文字起こしへ送信しませんでした。";
  return message;
}
