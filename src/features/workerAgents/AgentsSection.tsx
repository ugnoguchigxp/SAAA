import { useState } from "react";
import type { RevisionSummary, WorkerAgentSummary } from "../../lib/generated/workerAgents";
import { formatTime, type Labels } from "./labels";
import type { WorkerAgentsState } from "./useWorkerAgents";

type Props = {
  labels: Labels;
  language: string | undefined;
  state: WorkerAgentsState;
};

function RevisionLine({ labels, revision }: { labels: Labels; revision: RevisionSummary }) {
  return (
    <dl className="wa-revision">
      <dt>{labels.purpose}</dt>
      <dd>{revision.purpose}</dd>
      <dt>{labels.tools}</dt>
      <dd>{revision.toolKeys.length ? revision.toolKeys.join(", ") : labels.none}</dd>
      <dt>{labels.outputKind}</dt>
      <dd>{revision.outputKind}</dd>
    </dl>
  );
}

function ApproveDialog({
  labels,
  profileId,
  draft,
  state,
  onClose,
}: {
  labels: Labels;
  profileId: string;
  draft: RevisionSummary;
  state: WorkerAgentsState;
  onClose: () => void;
}) {
  return (
    <div className="wa-confirm" role="alertdialog" aria-label={labels.approveTitle}>
      <h4>{labels.approveTitle}</h4>
      <p className="wa-muted">{labels.approveNote}</p>
      <p>
        <strong>{profileId}</strong> ({labels.revisionNo(draft.revision)})
      </p>
      <RevisionLine labels={labels} revision={draft} />
      <p className="wa-muted wa-hash">
        {labels.hash}: {draft.definitionHash}
      </p>
      <div className="wa-actions">
        <button
          type="button"
          className="wa-primary"
          disabled={state.busy}
          onClick={async () => {
            // Stay open on failure so the reason stays next to the action that failed.
            if (await state.approve(profileId, draft.revisionId, draft.definitionHash)) onClose();
          }}
        >
          {labels.approveConfirm}
        </button>
        <button type="button" onClick={onClose}>
          {labels.cancel}
        </button>
      </div>
    </div>
  );
}

function AgentCard({ agent, labels, language, state }: Props & { agent: WorkerAgentSummary }) {
  const [confirming, setConfirming] = useState(false);
  const open = state.selected === agent.profileId;
  return (
    <li className="wa-card" data-profile={agent.profileId}>
      <div className="wa-card-head">
        <h3>{agent.profileId}</h3>
        <span className="wa-badge">{labels.origin[agent.origin] ?? agent.origin}</span>
        <label className="wa-toggle">
          <input
            type="checkbox"
            role="switch"
            aria-label={labels.enableToggle(agent.profileId)}
            checked={agent.enabled}
            disabled={state.busy}
            onChange={(event) => void state.setEnabled(agent.profileId, event.target.checked)}
          />
          {agent.enabled ? labels.enabled : labels.disabled}
        </label>
      </div>
      <h4>
        {labels.currentRevision}
        {agent.current ? ` (${labels.revisionNo(agent.current.revision)})` : ""}
      </h4>
      {agent.current ? (
        <RevisionLine labels={labels} revision={agent.current} />
      ) : (
        <p className="wa-muted">{labels.noCurrent}</p>
      )}
      {agent.draft ? (
        <div className="wa-draft">
          <h4>
            {labels.draftRevision} ({labels.revisionNo(agent.draft.revision)})
          </h4>
          <RevisionLine labels={labels} revision={agent.draft} />
          {confirming ? (
            <ApproveDialog
              labels={labels}
              profileId={agent.profileId}
              draft={agent.draft}
              state={state}
              onClose={() => setConfirming(false)}
            />
          ) : (
            <button type="button" disabled={state.busy} onClick={() => setConfirming(true)}>
              {labels.approve}
            </button>
          )}
        </div>
      ) : null}
      <button
        type="button"
        className="wa-link"
        aria-expanded={open}
        onClick={() => void state.selectAgent(open ? null : agent.profileId)}
      >
        {open ? labels.hideDetails : labels.details}
      </button>
      {open && state.detail?.profileId === agent.profileId ? (
        <table className="wa-table" aria-label={labels.revisions}>
          <thead>
            <tr>
              <th>#</th>
              <th>{labels.colState}</th>
              <th>{labels.purpose}</th>
              <th>{labels.tools}</th>
              <th>{labels.created}</th>
            </tr>
          </thead>
          <tbody>
            {state.detail.revisions.map((revision) => (
              <tr key={revision.revisionId} data-review-state={revision.reviewState}>
                <td>{revision.revision}</td>
                <td>{labels.reviewState[revision.reviewState] ?? revision.reviewState}</td>
                <td>{revision.purpose}</td>
                <td>{revision.toolKeys.join(", ") || labels.none}</td>
                <td>{formatTime(revision.createdAtMs, language)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </li>
  );
}

export function AgentsSection(props: Props) {
  const { labels, state } = props;
  return (
    <section className="wa-section" aria-label={labels.agents}>
      <h2>{labels.agents}</h2>
      {state.agents.length ? (
        <ul className="wa-list">
          {state.agents.map((agent) => (
            <AgentCard key={agent.profileId} agent={agent} {...props} />
          ))}
        </ul>
      ) : (
        <p className="wa-muted">{state.loaded ? labels.noAgents : labels.loading}</p>
      )}
    </section>
  );
}
