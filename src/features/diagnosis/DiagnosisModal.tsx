import { useTranslation } from "react-i18next";
import { useDialogFocus } from "../../components/useDialogFocus";
import type { DiagnosisItem } from "../../lib/generated/diagnosis";
import "./DiagnosisModal.css";
import { useDiagnosisReport } from "./useDiagnosisReport";

export function DiagnosisModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { report, running, error, rerun } = useDiagnosisReport();
  const { dialogRef, fallbackRef } = useDialogFocus(true, onClose);
  const groups = groupItems(report?.items ?? []);
  return (
    <div
      className="diagnosis-modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <aside
        ref={dialogRef}
        className="diagnosis-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="diagnosis-modal-title"
        tabIndex={-1}
      >
        <aside ref={fallbackRef} tabIndex={-1} hidden />
        <header className="diagnosis-modal-header">
          <div>
            <h2 id="diagnosis-modal-title">{t("chat.diagnosis.title")}</h2>
            <p className="diagnosis-modal-meta">
              {running
                ? t("chat.diagnosis.rerunning")
                : report
                  ? `revision ${report.revision}${report.finishedAt ? ` · ${report.finishedAt}` : ""}`
                  : t("chat.diagnosis.overall.running")}
            </p>
          </div>
          {(report || running) && (
            <span
              className="diagnosis-modal-badge"
              data-status={running ? "running" : report?.overall}
            >
              {t(`chat.diagnosis.overall.${running ? "running" : report?.overall}`)}
            </span>
          )}
        </header>
        <div className="diagnosis-modal-body">
          {groups.map(([group, items]) => (
            <section key={group} className="diagnosis-modal-group">
              <h3>{t(`chat.diagnosis.group.${group}`, { defaultValue: group })}</h3>
              {items.map((item) => (
                <article key={item.id} className="diagnosis-modal-item">
                  <span
                    className="diagnosis-modal-status"
                    data-status={item.status}
                    role="img"
                    aria-label={t(`chat.diagnosis.status.${item.status}`)}
                  />
                  <strong>
                    {t(`chat.diagnosis.items.${item.id}`, { defaultValue: item.label })}
                  </strong>
                  {(item.message || item.latencyMs != null) && (
                    <p>
                      {[item.message, item.latencyMs != null ? `${item.latencyMs} ms` : ""]
                        .filter(Boolean)
                        .join(" · ")}
                    </p>
                  )}
                </article>
              ))}
            </section>
          ))}
        </div>
        {error && <p className="diagnosis-modal-error">{error}</p>}
        <footer className="diagnosis-modal-footer">
          <button type="button" onClick={() => void rerun()} disabled={running}>
            {running ? t("chat.diagnosis.rerunning") : t("chat.diagnosis.rerun")}
          </button>
          <button type="button" onClick={onClose}>
            {t("chat.diagnosis.close")}
          </button>
        </footer>
      </aside>
    </div>
  );
}

function groupItems(items: DiagnosisItem[]) {
  const groups: Array<[string, DiagnosisItem[]]> = [];
  for (const item of items) {
    const current = groups[groups.length - 1];
    if (current && current[0] === item.group) current[1].push(item);
    else groups.push([item.group, [item]]);
  }
  return groups;
}
