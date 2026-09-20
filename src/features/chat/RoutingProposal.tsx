import type { RoutingSnapshot } from "../../lib/generated/runtimeEvent";

type RoutingProposalProps = {
  snapshot: RoutingSnapshot;
  cancellingRootId: string | null;
  onCancel: (rootId: string) => void;
};

/** Status comes from persisted receipts so a reload cannot create a second delegation. */
export function RoutingProposal({ snapshot, cancellingRootId, onCancel }: RoutingProposalProps) {
  const { active, queued } = snapshot;
  if (!active && queued.length === 0) return null;
  return (
    <aside className="routing-proposal" aria-live="polite">
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
    </aside>
  );
}
