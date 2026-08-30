import type { MutableRefObject } from "react";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { appendConversationActivity, type ConversationRuntimeActivity } from "../../lib/conversationActivity";
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
  setRuntimeActivity: (update: (current: ConversationRuntimeActivity[]) => ConversationRuntimeActivity[]) => void;
  stopSpeech: () => Promise<void>;
  submitPrompt: (prompt: string, options?: SubmitPromptOptions) => Promise<void>;
  transcribe?: typeof transcribeAudio;
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
        const transcript = await (context.transcribe ?? transcribeAudio)({
          runId,
          conversationId: segment.conversationId,
          samples: segment.samples,
          sampleRate: segment.sampleRate,
        }, (event) => {
          if (context.disposed.current || event.runId !== runId) return;
          if (event.type === "transcriptFinal") context.setTranscript(event.text);
        });
        clearSamples(segment);
        if (!transcript.trim()) continue;
        if (context.disposed.current) continue;
        context.setTranscript(transcript);
        if (context.conversation.current.speechRunId) await context.stopSpeech();
        if (context.disposed.current) continue;
        if (context.conversation.current.runId) {
          if (context.pendingPrompts.current.length >= 2) {
            context.setError(uiMessage("chatVoicePendingLimit"));
          } else {
            context.pendingPrompts.current.push({ content: transcript, inputOrigin: "voice" });
            context.setRuntimeActivity((current) => appendConversationActivity(current, { type: "voiceQueryQueued" }));
          }
        } else {
          void context.submitPrompt(transcript, { inputOrigin: "voice" });
        }
      } catch (cause) {
        clearSamples(segment);
        const message = toMessage(cause);
        if (!(segment.ttsActiveAtCapture && message.startsWith("TARGET_SPEAKER_REJECTED"))
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
  if (message.startsWith("TARGET_SPEAKER_REJECTED")) return uiMessage("voiceTargetSpeakerRejected");
  if (message.startsWith("ASR_LANGUAGE_NOT_ALLOWED")) return uiMessage("voiceAsrLanguageNotAllowed");
  if (message.startsWith("ASR_LANGUAGE_UNKNOWN")) return uiMessage("voiceAsrLanguageUnknown");
  if (message.startsWith("ASR_NO_SPEECH")) return uiMessage("voiceAsrNoSpeech");
  return message;
}
