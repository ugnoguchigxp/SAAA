import "./coding.css";
import { useEffect, useState } from "react";
import { codingApi, type CodingSnapshot } from "./api";
import { StewardPanel } from "./StewardPanel";
import { stewardErrorMessage } from "./stewardApi";
const labels: Record<string, string> = {
  queued: "受付済み",
  running: "実行中",
  cancel_requested: "停止受付済み",
  settled: "実行終了",
  failed: "実行失敗",
  interrupted: "停止確認済み",
  outcome_unknown: "実行状態の確認が必要",
};
export function CodingJobs({ conversationId }: { conversationId?: string }) {
  const [data, setData] = useState<CodingSnapshot | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [path, setPath] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    if (!conversationId) return;
    let live = true;
    const refresh = async () => {
      try {
        const [settings, next] = await Promise.all([
          codingApi.settings(),
          codingApi.snapshot(conversationId),
        ]);
        if (live) {
          setEnabled(settings.enabled);
          setData(next);
        }
      } catch (e) {
        if (live) setError(stewardErrorMessage(e));
      }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [conversationId]);
  async function select() {
    if (!conversationId) return;
    try {
      await codingApi.workspace(conversationId, path);
      setData(await codingApi.snapshot(conversationId));
      setError("");
    } catch (e) {
      setError(stewardErrorMessage(e));
    }
  }
  async function cancel(jobId: string, revision: number) {
    if (!conversationId) return;
    try {
      await codingApi.cancel(conversationId, jobId, revision);
      setData(await codingApi.snapshot(conversationId));
      setError("");
    } catch (e) {
      setError(stewardErrorMessage(e));
    }
  }
  if (!conversationId || (!enabled && !data?.jobs.length)) return null;
  return (
    <aside className="coding-jobs" aria-label="実装ジョブ">
      {enabled && (
        <details>
          <summary>実装先: {data?.workspace?.path ?? "未選択"}</summary>
          <label>
            ローカルGitフォルダーの絶対パス
            <input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="/Users/…/project"
            />
          </label>
          <button onClick={() => void select()} disabled={!path.trim()}>
            このフォルダーを選択
          </button>
          <p>選択後、会話で実装を依頼してください。</p>
        </details>
      )}
      {enabled && (
        <StewardPanel
          conversationId={conversationId}
          workspaceId={data?.workspace?.workspaceId}
          onError={setError}
        />
      )}
      {data?.jobs.map((job) => (
        <details key={job.jobId}>
          <summary>
            <strong>{labels[job.state] ?? job.state}</strong> <span>{job.workspace}</span>
          </summary>
          <p>{job.jobId}</p>
          {job.result?.summary && <p>{job.result.summary}</p>}
          {job.result?.error && <p role="alert">{job.result.error}</p>}
          {!!job.result?.errors && <p>モデル・ツールエラー: {job.result.errors}件</p>}
          {job.state === "settled" && <p>実行が終了しました。結果の確認は会話で依頼できます。</p>}
          {["queued", "running"].includes(job.state) && (
            <button onClick={() => void cancel(job.jobId, job.revision)}>実装を停止</button>
          )}
        </details>
      ))}
      {error && <p role="alert">{error}</p>}
    </aside>
  );
}
