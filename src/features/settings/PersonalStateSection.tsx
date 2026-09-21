import { useEffect, useState } from "react";
import { personalStateApi, type PersonalStateSnapshot } from "../memory/api";
import { RemoteCleanupStatus } from "./RemoteCleanupStatus";

export function PersonalStateSection() {
  const [snapshot, setSnapshot] = useState<PersonalStateSnapshot | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function refresh() {
    setBusy(true);
    try {
      setSnapshot(await personalStateApi.snapshot());
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  return (
    <section className="settings-section">
      <h3>メモリ</h3>
      <p>Personal Stateの動作状態を確認します。保存内容と根拠は上部の「メモリ」で確認できます。</p>
      <button type="button" onClick={() => void refresh()} disabled={busy}>
        更新
      </button>
      {error ? <p role="alert">{error}</p> : null}
      {snapshot ? (
        <>
          <p>
            抽出: {snapshot.enabled ? "有効" : "停止中"} / 接続:{" "}
            {snapshot.contractReady ? "契約確認済み" : "認定待ち"}
          </p>
          <p>
            未反映 {snapshot.pendingCount}件（{snapshot.pendingBytes} bytes） / 状態版{" "}
            {snapshot.revision}
          </p>
          {!snapshot.contractReady ? (
            <p>配送・消去・モデルの認定が完了するまで追加の推論を開始しません。</p>
          ) : null}
          <RemoteCleanupStatus operations={snapshot.remoteCleanup} />
          {snapshot.cleanup.some((item) => item.stage !== "complete") ? (
            <p>遠隔の削除確認が残っています。確認が終わるまで削除完了とは表示しません。</p>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
