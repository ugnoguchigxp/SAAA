import { useEffect, useRef, useState, type FormEvent } from "react";
import * as mediaApi from "./mediaApi";
import {
  mediaFailureMessage,
  mediaProgressMessage,
  type MediaKind,
  type MediaOutput,
  type MediaProgress,
} from "./mediaApi";
import "./mediaGeneration.css";

type MediaApi = Pick<typeof mediaApi, "generateMedia" | "cancelMedia" | "readMediaArtifact">;

export function MediaGenerationPanel({ api = mediaApi }: { api?: MediaApi } = {}) {
  const [kind, setKind] = useState<MediaKind>("image");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<MediaProgress | null>(null);
  const [output, setOutput] = useState<MediaOutput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<Record<number, string>>({});
  const [fetching, setFetching] = useState(false);
  const active = useRef<string | null>(null);
  const urls = useRef<string[]>([]);
  const downloaded = useRef(new Set<number>());
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      for (const url of urls.current) URL.revokeObjectURL(url);
      if (active.current) void api.cancelMedia(active.current).catch(() => {});
    };
  }, [api]);

  async function fetchArtifacts(result: MediaOutput) {
    if (!result.result) return;
    setFetching(true);
    setError(null);
    for (let index = 0; index < result.result.artifacts.length; index++) {
      if (downloaded.current.has(index)) continue;
      try {
        const bytes = await api.readMediaArtifact(result.runId, index);
        if (!alive.current) return;
        const artifact = result.result.artifacts[index];
        const url = URL.createObjectURL(new Blob([bytes], { type: artifact.mimeType }));
        urls.current.push(url);
        downloaded.current.add(index);
        setFiles((current) => ({ ...current, [index]: url }));
      } catch (cause) {
        if (alive.current) setError(String(cause));
      }
    }
    if (alive.current) setFetching(false);
  }
  async function submit(event?: FormEvent) {
    event?.preventDefault();
    if (busy || fetching || !prompt.trim()) return;
    for (const url of urls.current) URL.revokeObjectURL(url);
    urls.current = [];
    downloaded.current.clear();
    setFiles({});
    setOutput(null);
    setError(null);
    setBusy(true);
    const runId = crypto.randomUUID();
    active.current = runId;
    setProgress({ phase: "discovering", jobId: null, progress: null });
    try {
      const result = await api.generateMedia({ runId, kind, prompt }, (event) => {
        if (alive.current && active.current === runId) setProgress(event);
      });
      if (!alive.current) return;
      setOutput(result);
      if (result.result) await fetchArtifacts(result);
    } catch (cause) {
      if (alive.current) setError(String(cause));
    } finally {
      active.current = null;
      if (alive.current) setBusy(false);
    }
  }
  async function cancel() {
    if (!active.current) return;
    setProgress({ phase: "cancelling", jobId: progress?.jobId ?? null, progress: null });
    try {
      await api.cancelMedia(active.current);
    } catch (cause) {
      setError(String(cause));
    }
  }
  return (
    <details className="media-generation">
      <summary>画像・楽曲を作成</summary>
      <form onSubmit={(event) => void submit(event)}>
        <label>
          作成するもの
          <select
            aria-label="作成するもの"
            value={kind}
            disabled={busy || fetching}
            onChange={(event) => setKind(event.currentTarget.value as MediaKind)}
          >
            <option value="image">画像</option>
            <option value="music">楽曲</option>
          </select>
        </label>
        <textarea
          aria-label="生成する内容"
          value={prompt}
          disabled={busy || fetching}
          onChange={(event) => setPrompt(event.currentTarget.value)}
          placeholder="作りたい画像や楽曲を説明してください"
          rows={2}
        />
        <button
          type="submit"
          disabled={
            busy || fetching || !prompt.trim() || new TextEncoder().encode(prompt).length > 16384
          }
        >
          生成する
        </button>
        {busy && !fetching && active.current && (
          <button
            type="button"
            disabled={progress?.phase === "cancelling"}
            onClick={() => void cancel()}
          >
            中止
          </button>
        )}
      </form>
      {busy && !fetching && (
        <p role="status">
          {mediaProgressMessage(progress?.phase ?? "discovering")}
          {progress?.progress != null ? ` ${Math.round(progress.progress * 100)}%` : ""}
        </p>
      )}
      {output?.error && <p role="alert">{mediaFailureMessage(output.error)}</p>}
      {output?.error?.retryable && !busy && (
        <button type="button" onClick={() => void submit()}>
          生成を再試行
        </button>
      )}
      {error && <p role="alert">{error}</p>}
      {output?.result && (
        <div className="media-generation-artifacts">
          <p>生成成功 · {output.result.model}</p>
          {fetching && <p role="status">成果物を取得中…</p>}
          {output.result.artifacts.map((artifact, index) => (
            <div key={artifact.id}>
              {files[index] &&
                (output.result?.kind === "image" ? (
                  <img src={files[index]} alt={prompt} />
                ) : (
                  <audio controls src={files[index]} />
                ))}
              {files[index] && (
                <a
                  href={files[index]}
                  download={`${artifact.id}.${artifact.mimeType.split("/")[1] === "mpeg" ? "mp3" : artifact.mimeType.split("/")[1]}`}
                >
                  保存
                </a>
              )}
            </div>
          ))}
          {error && !fetching && (
            <button type="button" onClick={() => void fetchArtifacts(output)}>
              成果物の取得を再試行
            </button>
          )}
        </div>
      )}
    </details>
  );
}
