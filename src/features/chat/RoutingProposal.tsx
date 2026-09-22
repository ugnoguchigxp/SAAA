import { useTranslation } from "react-i18next";
import type { RoutingEventRecord, RoutingSnapshot } from "../../lib/generated/runtimeEvent";

type RoutingProposalProps = {
  snapshot: RoutingSnapshot;
  events?: RoutingEventRecord[];
  cancellingRootId: string | null;
  decidingProposalId: string | null;
  proposalError: string | null;
  onCancel: (rootId: string) => void;
  onDecideProposal: (proposalId: string, candidateId: string, approve: boolean) => void;
};

/** Status comes from persisted receipts so a reload cannot create a second delegation. */
export function RoutingProposal({
  snapshot,
  events = [],
  cancellingRootId,
  decidingProposalId,
  proposalError,
  onCancel,
  onDecideProposal,
}: RoutingProposalProps) {
  const { t } = useTranslation();
  const { active, queued, proposals } = snapshot;
  const cloudFallback = [active, ...queued].some((root) =>
    root?.decisionReasonCodes.includes("location_fallback"),
  );
  const recentEvents = events.slice(-12).reverse();
  if (!active && queued.length === 0 && proposals.length === 0 && recentEvents.length === 0)
    return null;
  return (
    <aside className="routing-proposal" aria-live="polite">
      {cloudFallback && <strong>{t("chat.locationFallback")}</strong>}
      {active && (
        <div>
          <span>
            Routing response in progress（{active.phase} / revision {active.revision}）
          </span>
          <button
            type="button"
            disabled={cancellingRootId === active.rootId}
            onClick={() => onCancel(active.rootId)}
          >
            {cancellingRootId === active.rootId ? "Stopping…" : "Stop routing"}
          </button>
        </div>
      )}
      {queued.length > 0 && (
        <small>
          {queued.length} routing request(s) queued（先頭: {queued[0]?.rootId}）
        </small>
      )}
      {proposals.map((proposal) => (
        <div key={proposal.id}>
          <span>
            Premium候補 {proposal.candidateId} / {proposal.status}
            {proposal.estimatedCostMicros === null
              ? " / 費用不明"
              : ` / 推定 ${proposal.estimatedCostMicros.toString()} μ`}
          </span>
          {proposal.status === "proposed" && (
            <>
              <button
                type="button"
                disabled={decidingProposalId !== null}
                onClick={() => onDecideProposal(proposal.id, proposal.candidateId, true)}
              >
                この候補を承認
              </button>
              <button
                type="button"
                disabled={decidingProposalId !== null}
                onClick={() => onDecideProposal(proposal.id, proposal.candidateId, false)}
              >
                辞退
              </button>
            </>
          )}
        </div>
      ))}
      {proposalError && <small role="alert">{proposalError}</small>}
      {recentEvents.length > 0 && (
        <details>
          <summary>Routing 実行履歴</summary>
          <ol>
            {recentEvents.map((event) => (
              <li key={`${event.rootId}:${event.seq.toString()}`}>
                <span>{routingEventLabel(event.kind)}</span>
                <small>
                  {event.rootId} / #{event.seq.toString()}
                  {routingEventReason(event.dataJson)}
                </small>
              </li>
            ))}
          </ol>
        </details>
      )}
    </aside>
  );
}

const routingEventLabels: Record<string, string> = {
  input_accepted: "入力を受け付けました",
  step_started: "担当処理を開始しました",
  step_completed: "担当処理が完了しました",
  review_completed: "独立レビューが完了しました",
  premium_proposed: "上位候補の承認を待っています",
  answer_committed: "最終回答を確定しました",
  root_finished: "Routing を終了しました",
  root_resumed: "Routing を再開しました",
  superseded: "新しい入力により旧結果を保留しました",
};

export function routingEventLabel(kind: string): string {
  return routingEventLabels[kind] ?? kind.replace(/_/g, " ");
}

export function routingEventReason(dataJson: string): string {
  try {
    const value: unknown = JSON.parse(dataJson);
    if (!value || typeof value !== "object" || Array.isArray(value)) return "";
    const record = value as Record<string, unknown>;
    const reason = [record.reasonCode, record.reason, record.candidateId, record.phase].find(
      (item): item is string => typeof item === "string" && item.length > 0,
    );
    if (!reason) return "";
    const normalized = reason.replace(/[\u0000-\u001f\u007f]/g, " ").trim();
    if (!normalized) return "";
    const visible = normalized.length > 160 ? `${normalized.slice(0, 159)}…` : normalized;
    return ` / ${visible}`;
  } catch {
    return "";
  }
}
