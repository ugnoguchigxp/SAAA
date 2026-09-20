import { z } from "zod";
import type { AppSnapshot, RuntimeEvent } from "./contracts";
import { runtimeFailureCodes } from "./generated/runtimeEvent";
import type { VoiceAsrStreamEvent } from "./generated/voiceAsr";

const text = z.string();
const id = text.min(1).max(1024);
const count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const pace = z.enum(["quick", "balanced", "patient"]);
const speechReason = z.enum([
  "global_opt_out",
  "conversation_override",
  "global_default",
  "situation_hold",
]);
const policy = z.object({
  conversationId: id,
  speechOutput: z.enum(["inherit", "muted"]),
  listeningPace: z.enum(["inherit", "quick", "balanced", "patient"]),
  policyRevision: count,
  updatedAt: text,
  effectiveSpeechOutput: z.enum(["speak", "silent"]),
  speechReasonCode: speechReason,
  effectiveListeningPace: pace,
  effectiveSilenceTimeoutMs: count,
});
const message = z.object({
  id,
  conversationId: id,
  role: z.enum(["user", "assistant", "system", "transcript"]),
  content: text,
  createdAt: text,
  parts: z
    .array(
      z.discriminatedUnion("type", [
        z.object({ type: z.literal("text"), text }),
        z.object({
          type: z.literal("ui"),
          instanceId: id,
          viewId: id,
          revision: count,
          summary: text,
        }),
      ]),
    )
    .max(10000)
    .optional(),
});
export const appSnapshotSchema = z.object({
  settings: z
    .array(
      z.object({
        namespace: z.enum([
          "providers.model",
          "providers.agent",
          "routing.tasks",
          "voice.runtime",
          "security.runtime",
          "ui.preferences",
          "situation.runtime",
          "routing.roles",
        ]),
        key: z.enum(["default", "codex-sdk"]),
        schemaVersion: z.union([z.literal(1), z.literal(15)]),
        valueJson: z.record(z.string(), z.unknown()),
        updatedAt: text,
      }),
    )
    .max(1000),
  conversations: z
    .array(
      z.object({
        id,
        title: text.nullable(),
        taskMode: z.enum(["conversation", "coding"]),
        createdAt: text,
        updatedAt: text,
      }),
    )
    .max(10000),
  primaryConversationId: text,
  effectiveRoute: z.object({
    providerId: text.nullable(),
    label: text,
    location: z.enum(["local", "cloud"]).nullable(),
    state: z.enum(["unchecked", "active", "ready", "failed"]),
    fallbackUsed: z.boolean(),
    reasonCode: text,
    updatedAt: text.nullable(),
  }),
  voiceProfile: z.object({
    status: z.enum(["empty", "collecting", "ready"]),
    filterEnabled: z.boolean(),
    runtimeAvailable: z.boolean(),
    runtimeMessage: text,
    sampleCount: count,
    targetSampleCount: count,
    totalDurationMs: count,
    minimumDurationMs: count,
    threshold: z.number().min(0).max(1),
    samples: z
      .array(
        z.object({
          id,
          ordinal: count,
          durationMs: count,
          inputDeviceId: text,
          effectiveAec: z.boolean(),
          createdAt: text,
        }),
      )
      .max(10000),
  }),
});
export const runtimeEventSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("started"), runId: id, route: text, providerId: text }),
  z.object({ type: z.literal("delta"), runId: id, text }),
  z.object({ type: z.literal("activity"), runId: id, kind: text, summary: text }),
  z.object({ type: z.literal("providerFailed"), runId: id, providerId: text, reason: text }),
  z.object({
    type: z.literal("messageCompleted"),
    runId: id,
    message,
    presentation: z.object({
      decision: z.enum(["speak", "silent"]),
      reasonCode: z.enum([
        "global_opt_out",
        "turn_override",
        "conversation_override",
        "global_default",
        "route_blocked",
        "situation_hold",
      ]),
    }),
    voicePolicy: policy.nullable(),
  }),
  z.object({ type: z.literal("speechStarted"), runId: id }),
  z.object({ type: z.literal("speechEnded"), runId: id }),
  z.object({ type: z.literal("speechFailed"), runId: id, message: text, recovery: text }),
  z.object({ type: z.literal("cancelled"), runId: id }),
  z.object({
    type: z.literal("failed"),
    runId: id,
    code: z.enum(runtimeFailureCodes),
    message: text,
    recovery: text,
  }),
]);
const asrCode = z.enum([
  "asr-session-exists",
  "asr-session-not-found",
  "asr-packet-format",
  "asr-packet-sequence",
  "asr-backpressure",
  "asr-provider-unavailable",
  "asr-stream-protocol",
  "asr-stream-timeout",
  "asr-final-timeout",
  "asr-target-speaker-unavailable",
  "asr-language-not-allowed",
  "asr-no-speech",
  "asr-cancelled",
]);
export const voiceAsrEventSchema = z
  .discriminatedUnion("type", [
    z.object({
      type: z.literal("ready"),
      sessionId: id,
      currentUtteranceId: id,
      protocol: z.literal("batch-agreement"),
      scope: z.enum(["all-speakers", "target-speaker"]),
    }),
    z.object({
      type: z.literal("partial"),
      sessionId: id,
      utteranceId: id,
      revision: count,
      startMs: count,
      endMs: count,
      stableText: text,
      unstableText: text,
      language: text.nullable(),
    }),
    z.object({
      type: z.literal("final"),
      sessionId: id,
      utteranceId: id,
      revision: count,
      startMs: count,
      endMs: count,
      text,
      language: text.nullable(),
    }),
    z.object({
      type: z.literal("utteranceDiscarded"),
      sessionId: id,
      utteranceId: id,
      reason: z.enum(["no-speech", "target-speaker-empty", "cancelled"]),
    }),
    z.object({
      type: z.literal("failed"),
      sessionId: id,
      utteranceId: text.nullable(),
      code: asrCode,
      message: text,
      recovery: text,
      fatal: z.boolean(),
    }),
    z.object({ type: z.literal("stopped"), sessionId: id }),
  ])
  .refine((event) => !("endMs" in event) || event.endMs >= event.startMs);

type Equal<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type Assert<T extends true> = T;
export type SnapshotSchemaContract = Assert<Equal<z.infer<typeof appSnapshotSchema>, AppSnapshot>>;
export type RuntimeSchemaContract = Assert<Equal<z.infer<typeof runtimeEventSchema>, RuntimeEvent>>;
export type VoiceAsrSchemaContract = Assert<
  Equal<z.infer<typeof voiceAsrEventSchema>, VoiceAsrStreamEvent>
>;

export type IpcBoundary = "snapshot" | "runtime" | "voice-asr";
export class IpcBoundaryError extends Error {
  constructor(public readonly boundary: IpcBoundary) {
    super(`Invalid ${boundary} IPC payload. Reload the application and retry.`);
  }
}
export function parseIpc<T>(schema: z.ZodType<T>, value: unknown, boundary: IpcBoundary): T {
  const parsed = schema.safeParse(value);
  if (!parsed.success) throw new IpcBoundaryError(boundary);
  return parsed.data;
}

/** Quarantine a broken subscription; never manufacture a successful terminal event. */
export function guardedReceiver<T>(
  schema: z.ZodType<T>,
  boundary: IpcBoundary,
  onEvent: (value: T) => void,
  validateOrder: (value: T) => boolean = () => true,
  onInvalid?: () => void,
) {
  let invalid = false;
  return (value: unknown) => {
    if (invalid) return;
    let event: T;
    try {
      event = parseIpc(schema, value, boundary);
      if (!validateOrder(event)) throw new IpcBoundaryError(boundary);
    } catch {
      invalid = true;
      if (typeof window !== "undefined")
        window.dispatchEvent(
          new window.CustomEvent("saaa:ipc-boundary-error", {
            detail: new IpcBoundaryError(boundary).message,
          }),
        );
      onInvalid?.();
      return;
    }
    onEvent(event);
  };
}
