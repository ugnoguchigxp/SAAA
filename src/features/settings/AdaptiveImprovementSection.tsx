import { useEffect, useState } from "react";
import {
  activateAdaptiveArtifact,
  approveAdaptiveArtifact,
  listAdaptiveEvaluations,
  rollbackAdaptiveArtifact,
} from "../../lib/roleRoutingApi";
import type { EvaluationView } from "../../lib/generated/runtimeEvent";

export function AdaptiveImprovementSection() {
  const [items, setItems] = useState<EvaluationView[]>([]);
  const [error, setError] = useState("");
  async function refresh() {
    try {
      setItems(await listAdaptiveEvaluations());
      setError("");
    } catch (caught) {
      setError(String(caught));
    }
  }
  useEffect(() => {
    void refresh();
  }, []);
  return (
    <section>
      <h3>適応改善の候補</h3>
      <p>
        通常の設定画面から評価結果を確認し、根拠がある候補だけを有効化します。集計値の手入力では昇格できません。
      </p>
      {error && <p>{error}</p>}
      {items.length === 0 && <p>表示できる評価済み候補はまだありません。</p>}
      {items.map((item) => {
        const canApprove = item.state === "shadow";
        const canActivate = item.state === "eligible";
        const canRollback = item.state === "active";
        const nextRevision = Number(item.policyRevision ?? 0n) + 1;
        const blocked =
          item.state === "evaluated"
            ? "実測が不足しているか、成功差の下限が正ではありません。"
            : item.state === "candidate"
              ? "評価記録がまだありません。"
              : item.state === "shadow"
                ? "shadow の観測が足りないと承認できません。"
                : "";
        return (
          <article key={item.artifactId}>
            <p>
              {item.domain} / {item.scopeKey} / {item.state}
              {item.groups ? <> / 母数 {item.groups} group</> : null}
              {item.successLower != null && <> / 成功差下限 {item.successLower}</>}
              {item.policyRevision != null && <> / 版 {item.policyRevision}</>}
            </p>
            {blocked && <p>{blocked}</p>}
            <button
              disabled={!canApprove}
              onClick={() =>
                void approveAdaptiveArtifact(item.artifactId, item.policyRevision).then(refresh)
              }
            >
              承認
            </button>
            <button
              disabled={!canActivate}
              onClick={() =>
                void activateAdaptiveArtifact(item.artifactId, nextRevision).then(refresh)
              }
            >
              有効化
            </button>
            <button
              disabled={!canRollback}
              onClick={() => void rollbackAdaptiveArtifact(item.artifactId).then(refresh)}
            >
              ルールに戻す
            </button>
          </article>
        );
      })}
    </section>
  );
}
