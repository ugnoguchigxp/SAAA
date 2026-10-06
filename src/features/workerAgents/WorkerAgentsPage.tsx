import { useTranslation } from "react-i18next";
import type { WebSearchMode } from "../../lib/generated/workerAgents";
import "./WorkerAgentsPage.css";
import { AgentsSection } from "./AgentsSection";
import { BlocklistSection } from "./BlocklistSection";
import { CreateDraftForm } from "./CreateDraftForm";
import { labelsFor } from "./labels";
import { TasksSection } from "./TasksSection";
import { useWorkerAgents } from "./useWorkerAgents";

const MODES: WebSearchMode[] = ["inline", "worker"];

export function WorkerAgentsPage() {
  const { i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const labels = labelsFor(language);
  const state = useWorkerAgents();

  return (
    <section className="worker-agents-page" aria-label={labels.title}>
      <div className="wa-content">
        <header className="wa-header">
          <div>
            <h1>{labels.title}</h1>
            <p className="wa-muted">{labels.intro}</p>
          </div>
          <button type="button" disabled={state.busy} onClick={() => void state.reload()}>
            {labels.reload}
          </button>
        </header>
        {state.error ? (
          <p className="wa-error" role="alert">
            {state.error}
          </p>
        ) : null}
        {state.busy && !state.loaded ? (
          <p className="wa-muted" role="status">
            {labels.loading}
          </p>
        ) : null}
        <section className="wa-section" aria-label={labels.webSearch}>
          <h2>{labels.webSearch}</h2>
          <p className="wa-muted" role="status" data-mode={state.mode ?? "unknown"}>
            {labels.modeCurrent}: {state.mode ?? labels.modeUnknown}
          </p>
          <div className="wa-actions" role="radiogroup" aria-label={labels.webSearch}>
            {MODES.map((mode) => (
              <button
                key={mode}
                type="button"
                role="radio"
                aria-checked={state.mode === mode}
                className={state.mode === mode ? "wa-primary" : ""}
                disabled={state.busy}
                onClick={() => void state.chooseMode(mode)}
              >
                {mode === "inline" ? labels.modeInline : labels.modeWorker}
              </button>
            ))}
          </div>
          <p className="wa-muted">{labels.modeInlineHelp}</p>
          <p className="wa-muted">{labels.modeWorkerHelp}</p>
        </section>
        <AgentsSection labels={labels} language={language} state={state} />
        <CreateDraftForm labels={labels} busy={state.busy} onSave={state.saveDraft} />
        <TasksSection labels={labels} language={language} state={state} />
        <BlocklistSection labels={labels} language={language} state={state} />
      </div>
    </section>
  );
}

export default WorkerAgentsPage;
