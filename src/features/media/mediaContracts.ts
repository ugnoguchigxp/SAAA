import { z } from "zod";

export const kindSchema = z.enum(["image", "music"]);
export const artifactSchema = z.object({
  id: z.string(),
  contentUrl: z.string(),
  metadataUrl: z.string().nullable(),
  mimeType: z.enum([
    "image/png",
    "image/webp",
    "image/jpeg",
    "audio/ogg",
    "audio/mpeg",
    "audio/wav",
    "audio/flac",
  ]),
  metadata: z.unknown(),
});
export const resultSchema = z.object({
  kind: kindSchema,
  model: z.string(),
  jobId: z.string().nullable(),
  artifacts: z.array(artifactSchema).min(1).max(8),
});
export const failureSchema = z.object({
  kind: z.enum([
    "discovery",
    "conflict",
    "startupFailed",
    "generationFailed",
    "timeout",
    "cancelled",
    "outcomeUnknown",
    "artifactFailed",
    "protocol",
  ]),
  code: z.string(),
  retryable: z.boolean(),
  mayHaveGenerated: z.boolean(),
  jobId: z.string().nullable(),
});
export const outputSchema = z
  .object({ runId: z.string(), result: resultSchema.nullable(), error: failureSchema.nullable() })
  .refine(
    (value) => Boolean(value.result) !== Boolean(value.error),
    "生成結果または失敗情報が必要です。",
  );
export const progressSchema = z.object({
  phase: z.string(),
  jobId: z.string().nullable(),
  progress: z.number().min(0).max(1).nullable(),
});
export type MediaKind = z.infer<typeof kindSchema>;
export type MediaResult = z.infer<typeof resultSchema>;
export type MediaFailure = z.infer<typeof failureSchema>;
export type MediaProgress = z.infer<typeof progressSchema>;
export type MediaOutput = z.infer<typeof outputSchema>;
