import { z } from "zod";

import {
  failureSchema,
  kindSchema,
  outputSchema,
  progressSchema,
  resultSchema,
  type MediaFailure,
  type MediaKind,
  type MediaOutput,
  type MediaProgress,
  type MediaResult,
} from "./mediaContracts";

export type { MediaFailure, MediaKind, MediaOutput, MediaProgress, MediaResult };

export const historySchema = z.array(
  z.object({
    runId: z.string(),
    kind: kindSchema.nullable(),
    connectionLabel: z.string().nullable(),
    model: z.string().nullable(),
    status: z.string(),
    jobId: z.string().nullable(),
    result: resultSchema.nullable(),
    error: failureSchema.nullable(),
    updatedAt: z.string(),
  }),
);
export type MediaHistory = z.infer<typeof historySchema>;

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

export type MediaApi = {
  generateMedia: (
    input: { runId: string; kind: MediaKind; prompt: string },
    onProgress: (value: MediaProgress) => void,
  ) => Promise<MediaOutput>;
  cancelMedia: (runId: string) => Promise<void>;
  readMediaArtifact: (runId: string, artifactIndex: number) => Promise<ArrayBuffer>;
  listMediaGenerations?: () => Promise<MediaHistory>;
  reconcileMedia?: (
    runId: string,
    onProgress: (value: MediaProgress) => void,
  ) => Promise<MediaOutput>;
};

export function sameRun(output: MediaOutput, runId: string, kind?: MediaKind): void {
  if (output.runId !== runId || (output.result && kind && output.result.kind !== kind)) {
    throw new Error("生成結果の識別子が一致しません。");
  }
}

export function parseOutput(value: unknown, runId: string, kind?: MediaKind): MediaOutput {
  const output = outputSchema.parse(value);
  sameRun(output, runId, kind);
  return output;
}

export function parseProgress(value: unknown): MediaProgress | null {
  const parsed = progressSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
