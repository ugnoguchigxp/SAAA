import { isActiveTask } from "./api";
import { formatTime, type Labels } from "./labels";
import type { WorkerAgentsState } from "./useWorkerAgents";

export function TasksSection({
  labels,
  language,
  state,
}: {
  labels: Labels;
  language: string | undefined;
  state: WorkerAgentsState;
}) {
  return (
    <section className="wa-section" aria-label={labels.tasks}>
      <h2>{labels.tasks}</h2>
      {state.tasks.length ? (
        <table className="wa-table">
          <thead>
            <tr>
              <th>{labels.colProfile}</th>
              <th>{labels.colState}</th>
              <th>{labels.colDelivery}</th>
              <th>{labels.colFailure}</th>
              <th>{labels.colCreated}</th>
              <th>{labels.colUpdated}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {state.tasks.map((task) => (
              <tr key={task.taskId} data-task={task.taskId}>
                <td>{task.profileId}</td>
                <td>{task.state}</td>
                <td>{task.delivery}</td>
                <td>{task.failureCode ?? "-"}</td>
                <td>{formatTime(task.createdAtMs, language)}</td>
                <td>{formatTime(task.updatedAtMs, language)}</td>
                <td>
                  {isActiveTask(task.state) ? (
                    <button
                      type="button"
                      disabled={state.busy}
                      aria-label={labels.cancelTaskLabel(task.taskId)}
                      onClick={() => void state.cancelTask(task.taskId)}
                    >
                      {labels.cancelTask}
                    </button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p className="wa-muted">{state.loaded ? labels.noTasks : labels.loading}</p>
      )}
    </section>
  );
}
