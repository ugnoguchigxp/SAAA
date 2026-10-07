import { historySchema, parseOutput, type MediaApi, type MediaHistory } from "./mediaApiModel";
import { MediaStreamError, readMediaNdjson } from "./mediaHttpStream";

export function createHttpMediaApi(base = ""): MediaApi {
  return {
    generateMedia: async (input, onProgress) => {
      const response = await postJson(`${base}/api/v1/media/runs`, input);
      try {
        return await readBody(response, input.runId, onProgress);
      } catch (error) {
        const recovered = await readOne(base, input.runId);
        if (recovered) return recovered;
        throw error;
      }
    },
    cancelMedia: async (runId) => {
      const response = await fetch(
        `${base}/api/v1/media/runs/${encodeURIComponent(runId)}/cancel`,
        {
          method: "POST",
          credentials: "same-origin",
        },
      );
      if (response.status !== 202) throw await responseError(response);
    },
    readMediaArtifact: async (runId, artifactIndex) => {
      const response = await fetch(
        `${base}/api/v1/media/runs/${encodeURIComponent(runId)}/artifacts/${artifactIndex}`,
        { credentials: "same-origin" },
      );
      if (!response.ok) throw await responseError(response);
      return response.arrayBuffer();
    },
    listMediaGenerations: () => listRuns(base),
    reconcileMedia: async (runId, onProgress) => {
      const response = await fetch(
        `${base}/api/v1/media/runs/${encodeURIComponent(runId)}/reconcile`,
        { method: "POST", credentials: "same-origin" },
      );
      if (!response.ok) throw await responseError(response);
      try {
        return await readBody(response, runId, onProgress);
      } catch (error) {
        const recovered = await readOne(base, runId);
        if (recovered) return recovered;
        throw error;
      }
    },
  };
}

async function postJson(url: string, body: unknown): Promise<Response> {
  const response = await fetch(url, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await responseError(response);
  return response;
}

async function readBody(
  response: Response,
  runId: string,
  onProgress: Parameters<MediaApi["generateMedia"]>[1],
) {
  if (!response.body) throw new MediaStreamError("生成応答を読み取れませんでした。");
  return readMediaNdjson(response.body, runId, onProgress);
}

async function listRuns(base: string, runId?: string): Promise<MediaHistory> {
  const query = runId ? `?runId=${encodeURIComponent(runId)}` : "";
  const response = await fetch(`${base}/api/v1/media/runs${query}`, { credentials: "same-origin" });
  if (!response.ok) throw await responseError(response);
  return historySchema.parse(await response.json());
}

async function readOne(base: string, runId: string) {
  try {
    const rows = await listRuns(base, runId);
    const row = rows.find((item) => item.runId === runId);
    if (!row?.result && !row?.error) return null;
    return parseOutput({ runId, result: row?.result ?? null, error: row?.error ?? null }, runId);
  } catch {
    return null;
  }
}

async function responseError(response: Response): Promise<Error> {
  const body = (await response.json().catch(() => null)) as {
    error?: { message?: unknown };
  } | null;
  const message = body?.error?.message;
  if (typeof message === "string" && message.length > 0) return new Error(message);
  return new Error(`生成要求を完了できませんでした（${response.status}）。`);
}
