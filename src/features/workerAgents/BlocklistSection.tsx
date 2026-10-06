import { formatTime, type Labels } from "./labels";
import type { WorkerAgentsState } from "./useWorkerAgents";

export function BlocklistSection({
  labels,
  language,
  state,
}: {
  labels: Labels;
  language: string | undefined;
  state: WorkerAgentsState;
}) {
  return (
    <section className="wa-section" aria-label={labels.blocklist}>
      <h2>{labels.blocklist}</h2>
      <p className="wa-muted">{labels.blocklistNote}</p>
      {state.blocklist.length ? (
        <table className="wa-table">
          <thead>
            <tr>
              <th>{labels.colHost}</th>
              <th>{labels.colReason}</th>
              <th>{labels.colCreated}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {state.blocklist.map((entry) => (
              <tr key={entry.urlHash} data-url-hash={entry.urlHash}>
                <td>{entry.host}</td>
                <td>{entry.reason}</td>
                <td>{formatTime(entry.createdAtMs, language)}</td>
                <td>
                  <button
                    type="button"
                    disabled={state.busy}
                    aria-label={labels.removeLabel(entry.host)}
                    onClick={() => void state.removeBlocked(entry.urlHash)}
                  >
                    {labels.remove}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="wa-muted">{state.loaded ? labels.noBlocklist : labels.loading}</p>
      )}
    </section>
  );
}
