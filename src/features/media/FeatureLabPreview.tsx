import { useMemo, useState } from "react";
import { LightAvatarBackground } from "../chat/avatar/LightAvatarBackground";
import { createHttpMediaApi } from "./mediaHttpApi";
import { MediaGenerationPanel } from "./MediaGenerationPanel";
import {
  type MediaApi,
  type MediaHistory,
  type MediaOutput,
  type MediaProgress,
} from "./mediaApiModel";

export const previewScenes = ["success", "progress", "cancelled", "unknown", "history"] as const;
export type PreviewScene = (typeof previewScenes)[number];

const artifact = {
  id: "preview-artifact",
  contentUrl: "/preview/content",
  metadataUrl: null,
  mimeType: "image/png" as const,
  metadata: {},
};

function output(scene: PreviewScene, runId: string): MediaOutput {
  if (scene === "cancelled") {
    return {
      runId,
      result: null,
      error: {
        kind: "cancelled",
        code: "cancelled_before_submission",
        retryable: false,
        mayHaveGenerated: false,
        jobId: null,
      },
    };
  }
  if (scene === "unknown") {
    return {
      runId,
      result: null,
      error: {
        kind: "outcomeUnknown",
        code: "synchronous_image_has_no_job",
        retryable: false,
        mayHaveGenerated: true,
        jobId: null,
      },
    };
  }
  return {
    runId,
    result: { kind: "image", model: "preview-model", jobId: null, artifacts: [artifact] },
    error: null,
  };
}

const history: MediaHistory = [
  {
    runId: "00000000-0000-4000-8000-000000000001",
    kind: "image",
    connectionLabel: "preview",
    model: "preview-model",
    status: "unknown",
    jobId: null,
    result: null,
    error: {
      kind: "outcomeUnknown",
      code: "synchronous_image_has_no_job",
      retryable: false,
      mayHaveGenerated: true,
      jobId: null,
    },
    updatedAt: "2026-10-07T00:00:00.000Z",
  },
];

/** 1×1 red PNG. A truncated signature is not a decodable image. */
const PREVIEW_PNG = [
  137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0,
  0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0, 3, 1, 1, 0,
  201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

export function previewMediaApi(scene: PreviewScene): MediaApi {
  let stopProgress: ((output: MediaOutput) => void) | undefined;
  return {
    generateMedia: (input, onProgress) => {
      const progress: MediaProgress = { phase: "starting", jobId: null, progress: null };
      onProgress(progress);
      if (scene !== "progress") return Promise.resolve(output(scene, input.runId));
      return new Promise((resolve) => {
        stopProgress = resolve;
      });
    },
    cancelMedia: async (runId) => {
      stopProgress?.(output("cancelled", runId));
      stopProgress = undefined;
    },
    readMediaArtifact: async () => Uint8Array.from(PREVIEW_PNG).buffer,
    listMediaGenerations: async () => (scene === "history" ? history : []),
    reconcileMedia: async (runId) => output("unknown", runId),
  };
}

export function FeatureLabPreview({
  scene = "success",
  transport = "react-mock",
  provider = "fixture",
}: {
  scene?: PreviewScene;
  transport?: "react-mock" | "http";
  provider?: "fixture" | "larm";
}) {
  const [current, setCurrent] = useState<PreviewScene>(scene);
  const http = transport === "http";
  const api = useMemo(
    () => (http ? createHttpMediaApi() : previewMediaApi(current)),
    [current, http],
  );
  const mode = http
    ? provider === "larm"
      ? "実LARMへ送る設定です。起動だけでは生成しません。"
      : "HTTPと保存を通る試用です。実サービスには送っていません。"
    : "画面だけの確認です。生成サービスには送っていません。";
  return (
    <main>
      <p>アバターは描画だけの確認です。音声と推論の成功は示しません。</p>
      <p>{mode}</p>
      {http ? null : (
        <label>
          場面
          <select
            aria-label="場面"
            value={current}
            onChange={(event) => setCurrent(event.target.value as PreviewScene)}
          >
            {previewScenes.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
        </label>
      )}
      <div style={{ position: "relative", height: 160, overflow: "hidden" }}>
        <LightAvatarBackground
          active
          cue={{ id: "preview-cue", motion: "neutral" }}
          reduced={false}
        />
      </div>
      <MediaGenerationPanel key={current} api={api} fixedKind="image" embedded />
    </main>
  );
}
