import { Channel, invoke } from "@tauri-apps/api/core";

import {
  historySchema,
  mediaFailureMessage,
  mediaProgressMessage,
  parseOutput,
  parseProgress,
  type MediaApi,
  type MediaHistory,
  type MediaKind,
  type MediaOutput,
  type MediaProgress,
} from "./mediaApiModel";

export type { MediaApi, MediaHistory, MediaKind, MediaOutput, MediaProgress };
export type { MediaFailure, MediaResult } from "./mediaApiModel";
export { historySchema, mediaFailureMessage, mediaProgressMessage };

export const desktopMediaApi: MediaApi = {
  generateMedia,
  cancelMedia,
  readMediaArtifact,
  listMediaGenerations,
  reconcileMedia,
};

export async function generateMedia(
  input: { runId: string; kind: MediaKind; prompt: string },
  onProgress: (value: MediaProgress) => void,
): Promise<MediaOutput> {
  const channel = new Channel<unknown>();
  channel.onmessage = (value) => {
    const parsed = parseProgress(value);
    if (parsed) onProgress(parsed);
  };
  return parseOutput(
    await invoke<unknown>("generate_media", { input, onProgress: channel }),
    input.runId,
    input.kind,
  );
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

export async function listMediaGenerations(): Promise<MediaHistory> {
  return historySchema.parse(await invoke("list_media_generations"));
}
export async function reconcileMedia(
  runId: string,
  onProgress: (value: MediaProgress) => void,
): Promise<MediaOutput> {
  const channel = new Channel<unknown>();
  channel.onmessage = (value) => {
    const parsed = parseProgress(value);
    if (parsed) onProgress(parsed);
  };
  return parseOutput(
    await invoke("reconcile_media_generation", { runId, onProgress: channel }),
    runId,
  );
}
