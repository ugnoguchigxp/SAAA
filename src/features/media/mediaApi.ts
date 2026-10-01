import { Channel, invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const kindSchema = z.enum(["image", "music"]);
const artifactSchema = z.object({
  id: z.string(),
  contentUrl: z.string(),
  metadataUrl: z.string().nullable(),
  mimeType: z.enum(["image/png", "image/webp", "audio/mpeg", "audio/wav", "audio/flac"]),
  metadata: z.unknown(),
});
const resultSchema = z.object({
  kind: kindSchema,
  model: z.string(),
  jobId: z.string().nullable(),
  artifacts: z.array(artifactSchema).min(1).max(8),
});
const failureSchema = z.object({
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
const outputSchema = z
  .object({ runId: z.string(), result: resultSchema.nullable(), error: failureSchema.nullable() })
  .refine(
    (value) => Boolean(value.result) !== Boolean(value.error),
    "生成結果または失敗情報が必要です。",
  );
const progressSchema = z.object({
  phase: z.string(),
  jobId: z.string().nullable(),
  progress: z.number().min(0).max(1).nullable(),
});
export type MediaKind = z.infer<typeof kindSchema>;
export type MediaResult = z.infer<typeof resultSchema>;
export type MediaFailure = z.infer<typeof failureSchema>;
export type MediaProgress = z.infer<typeof progressSchema>;
export type MediaOutput = z.infer<typeof outputSchema>;

export async function generateMedia(
  input: { runId: string; kind: MediaKind; prompt: string },
  onProgress: (value: MediaProgress) => void,
): Promise<MediaOutput> {
  const channel = new Channel<unknown>();
  channel.onmessage = (value) => {
    const parsed = progressSchema.safeParse(value);
    if (parsed.success) onProgress(parsed.data);
  };
  const output = outputSchema.parse(
    await invoke<unknown>("generate_media", { input, onProgress: channel }),
  );
  if (output.runId !== input.runId || (output.result && output.result.kind !== input.kind))
    throw new Error("生成結果の識別子が一致しません。");
  return output;
}
export async function cancelMedia(runId: string): Promise<void> {
  await invoke("cancel_media_generation", { runId });
}
export async function readMediaArtifact(
  runId: string,
  artifactIndex: number,
): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("read_generated_media", { runId, artifactIndex });
}

export function mediaFailureMessage(failure: MediaFailure): string {
  const labels: Record<MediaFailure["kind"], string> = {
    discovery: "生成サービスを確認できませんでした。",
    conflict: "他の重量級サービスと競合し、生成に失敗しました。空いた後で再試行できます。",
    startupFailed: "生成モデルを起動できませんでした。",
    generationFailed: "生成に失敗しました。",
    timeout: "生成の待機時間を超えました。",
    cancelled: "生成の待機を中止しました。",
    outcomeUnknown: "生成の状態を確認できませんでした。",
    artifactFailed: "生成後の成果物を取得できませんでした。",
    protocol: "生成サービスの応答を確認できませんでした。",
  };
  const uncertainty = failure.mayHaveGenerated
    ? "サーバーでは処理が続いている可能性があります。同じ内容の再送は重複生成になる場合があります。"
    : "";
  return `${labels[failure.kind]}${uncertainty}${failure.jobId ? ` ジョブ: ${failure.jobId}` : ""}`;
}
export function mediaProgressMessage(phase: string): string {
  const labels: Record<string, string> = {
    discovering: "生成サービスを確認中…",
    starting: "モデルの起動・生成を待っています。起動には数分かかる場合があります。",
    queued: "楽曲の生成待ち…",
    loading: "楽曲モデルを起動中…",
    generating: "楽曲を生成中…",
    encoding: "音声ファイルを作成中…",
    cancelling: "中止を受け付けました。ジョブを確認して停止を依頼しています…",
    completed: "生成が完了しました。成果物を取得しています…",
  };
  return labels[phase] ?? "生成を処理中…";
}
