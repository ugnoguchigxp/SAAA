import type { MediaHistory } from "./mediaApi";

function updatedAtLabel(value: string): string {
  const parsed = /^\d+$/.test(value) ? Number(value) : Date.parse(value);
  return Number.isFinite(parsed) ? new Date(parsed).toLocaleString() : value;
}

const statusLabel: Record<string, string> = {
  reserved: "受付準備中",
  submitting: "受付を確認中",
  running: "生成中",
  starting: "開始待ち",
  processing: "生成中",
  cancel_requested: "停止を確認中",
  accepted: "完了",
  cancelled: "中止確認済み",
  unknown: "結果未確認",
  failed: "失敗",
};
export function MediaHistoryList({
  history,
  error,
  busy,
  canReconcile,
  recover,
}: {
  history: MediaHistory;
  error: string | null;
  busy: boolean;
  canReconcile: boolean;
  recover: (record: MediaHistory[number]) => Promise<void>;
}) {
  return (
    <>
      {error && <p role="status">生成履歴を取得できませんでした: {error}</p>}{" "}
      {history.length > 0 && (
        <details>
          <summary>保存済みの生成履歴</summary>
          <p>照会では保存した処理IDの結果を取得します。生成要求は再送しません。</p>
          {history
            .filter((record) => record.kind !== null)
            .map((record) => (
              <div key={record.runId}>
                <p>
                  {record.kind === "image" ? "画像" : "楽曲"} · {record.connectionLabel} /{" "}
                  {record.model} · {statusLabel[record.status] ?? "処理を確認中"}（
                  {updatedAtLabel(record.updatedAt)}）
                </p>
                {(record.result || (record.jobId && canReconcile)) && (
                  <button type="button" disabled={busy} onClick={() => void recover(record)}>
                    {record.result ? "成果物を開く" : "進行状況を照会"}
                  </button>
                )}
                {!record.result && !record.jobId && record.status !== "cancelled" && (
                  <p>処理IDが不明です。サービス側の履歴を確認してください。</p>
                )}
              </div>
            ))}
        </details>
      )}
    </>
  );
}
