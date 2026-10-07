import { MediaGenerationForm } from "./MediaGenerationForm";
import { MediaHistoryList } from "./MediaHistoryList";
import { MediaArtifacts } from "./MediaArtifacts";
import { useEffect, useRef, useState, type FormEvent } from "react";
import * as mediaApi from "./mediaApi";
import {
  mediaFailureMessage,
  mediaProgressMessage,
  type MediaApi,
  type MediaKind,
  type MediaOutput,
  type MediaProgress,
} from "./mediaApiModel";
import "./mediaGeneration.css";

export function MediaGenerationPanel({
  api = mediaApi.desktopMediaApi,
  fixedKind,
  embedded = false,
  onBusyChange,
}: {
  api?: MediaApi;
  fixedKind?: MediaKind;
  embedded?: boolean;
  onBusyChange?: (busy: boolean) => void;
} = {}) {
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [history, setHistory] = useState<mediaApi.MediaHistory>([]);
  const [kind, setKind] = useState<MediaKind>(fixedKind ?? "image");
  const [prompt, setPrompt] = useState(
    fixedKind === "image"
      ? "白い背景に青い円を描いた、シンプルなイラスト"
      : fixedKind === "music"
        ? "穏やかなピアノを中心にした、落ち着いた楽曲"
        : "",
  );
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<MediaProgress | null>(null);
  const [output, setOutput] = useState<MediaOutput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<Record<number, string>>({});
  const [fetching, setFetching] = useState(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const active = useRef<string | null>(null);
  const urls = useRef<string[]>([]);
  const downloaded = useRef(new Set<number>());
  const alive = useRef(true);
  useEffect(() => {
    onBusyChange?.(busy || fetching);
    return () => onBusyChange?.(false);
  }, [busy, fetching, onBusyChange]);
  useEffect(() => {
    if (!busy) return;
    const started = Date.now();
    setElapsedSeconds(0);
    const timer = window.setInterval(
      () => setElapsedSeconds(Math.floor((Date.now() - started) / 1000)),
      1000,
    );
    return () => window.clearInterval(timer);
  }, [busy]);
  useEffect(() => {
    alive.current = true;
    void api
      .listMediaGenerations?.()
      .then((records) => {
        if (alive.current) setHistory(records);
      })
      .catch((cause) => {
        if (alive.current) setHistoryError(String(cause));
      });
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
    if (active.current || busy || fetching || !prompt.trim()) return;
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
      if (alive.current) {
        setBusy(false);
        void api
          .listMediaGenerations?.()
          .then((records) => {
            if (alive.current) setHistory(records);
          })
          .catch(() => {});
      }
    }
  }
  async function recover(record: mediaApi.MediaHistory[number]) {
    if (busy || fetching) return;
    for (const url of urls.current) URL.revokeObjectURL(url);
    urls.current = [];
    downloaded.current.clear();
    setFiles({});
    setError(null);
    setOutput(null);
    setBusy(true);
    active.current = record.runId;
    try {
      const result = record.result
        ? { runId: record.runId, result: record.result, error: null }
        : await api.reconcileMedia!(record.runId, (event) => {
            if (alive.current) setProgress(event);
          });
      if (alive.current) {
        setOutput(result);
        if (result.result) await fetchArtifacts(result);
      }
    } catch (cause) {
      if (alive.current) setError(String(cause));
    } finally {
      active.current = null;
      if (alive.current) {
        setBusy(false);
        void api
          .listMediaGenerations?.()
          .then((records) => {
            if (alive.current) setHistory(records);
          })
          .catch(() => {});
      }
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
  const Wrapper = embedded ? "div" : "details";
  return (
    <Wrapper className="media-generation">
      {!embedded && <summary>画像・楽曲を作成</summary>}
      {embedded && (
        <p>
          依頼後にモデルを起動します。完了まで数分以上かかる場合があります。自動再送はしません。
        </p>
      )}
      <MediaGenerationForm
        kind={kind}
        lockKind={fixedKind !== undefined}
        prompt={prompt}
        busy={busy || fetching}
        canCancel={busy && !fetching && !!active.current}
        cancelling={progress?.phase === "cancelling"}
        setKind={setKind}
        setPrompt={setPrompt}
        submit={submit}
        cancel={cancel}
      />
      {busy && !fetching && (
        <p role="status">
          {mediaProgressMessage(progress?.phase ?? "discovering")}
          {progress?.progress != null ? ` ${Math.round(progress.progress * 100)}%` : ""}
          {`（経過 ${elapsedSeconds} 秒）`}
          {progress?.jobId && ` ジョブ: ${progress.jobId}`}
        </p>
      )}
      <MediaHistoryList
        error={historyError}
        history={history}
        busy={busy || fetching}
        canReconcile={!!api.reconcileMedia}
        recover={recover}
      />
      {output?.error && (
        <p role="alert">
          {mediaFailureMessage(output.error)}
          {` (コード: ${output.error.code})`}
        </p>
      )}
      {output?.error?.retryable && !busy && (
        <button type="button" onClick={() => void submit()}>
          生成を再試行
        </button>
      )}
      {error && <p role="alert">{error}</p>}
      {output?.result && (
        <MediaArtifacts
          output={output}
          files={files}
          prompt={prompt}
          fetching={fetching}
          error={error}
          retry={() => fetchArtifacts(output)}
        />
      )}
    </Wrapper>
  );
}
