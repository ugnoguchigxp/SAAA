import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import "./DiagnosisPage.css";
import { CapabilityCard, useAgeText } from "./CapabilityCard";
import { isStale, sortCapabilities, summarize } from "./diagnosisModel";
import { type DiagnosisBackend, useDiagnosisReport } from "./useDiagnosisReport";

export function DiagnosisPage({
  onOpenSettings,
  backend,
}: {
  onOpenSettings?: () => void;
  backend?: DiagnosisBackend;
}) {
  const { t } = useTranslation();
  const ageText = useAgeText();
  const { report, loaded, running, error, now, run } = useDiagnosisReport(backend);
  const refreshed = useRef(false);

  // Opening the page shows current facts: refresh stale state with a quick scan once.
  useEffect(() => {
    if (!loaded || refreshed.current) return;
    refreshed.current = true;
    if (isStale(report, Date.now()) && report?.running !== true) void run({ kind: "quick" });
  }, [loaded, report, run]);

  const ran = report != null && report.revision > 0;
  const overall = ran ? report.overall : "unverified";
  const summary = ran ? summarize(report) : null;
  const headline = !ran
    ? t("chat.diagnosis.headline.none")
    : t(`chat.diagnosis.headline.${overall}`, { count: summary?.unverified ?? 0 });
  const needsLiveCheck = ran && (overall === "unverified" || (summary?.unverified ?? 0) > 0);

  return (
    <section className="diagnosis-page" aria-label={t("navigation.diagnosis")}>
      <div className="dx-content">
        <header className={`dx-banner dx-state-${overall}`} data-state={overall}>
          <div className="dx-banner-text">
            <p className="dx-eyebrow">{t("chat.diagnosis.eyebrow")}</p>
            <h2>{headline}</h2>
            <p className="dx-muted" role="status" aria-live="polite">
              {running
                ? t("chat.diagnosis.running")
                : ran && report.finishedAt
                  ? t("chat.diagnosis.lastRun", { age: ageText(now, report.finishedAt) })
                  : t("chat.diagnosis.neverRun")}
            </p>
          </div>
          <div className="dx-banner-actions">
            <button
              type="button"
              className={needsLiveCheck ? "" : "dx-primary"}
              disabled={running}
              onClick={() => void run({ kind: "quick" })}
              title={t("chat.diagnosis.quickHint")}
            >
              {t("chat.diagnosis.quick")}
            </button>
            <button
              type="button"
              className={needsLiveCheck ? "dx-primary" : ""}
              disabled={running}
              onClick={() => void run({ kind: "full" })}
              title={t("chat.diagnosis.fullHint")}
            >
              {t("chat.diagnosis.full")}
            </button>
          </div>
        </header>
        {error ? (
          <p className="dx-error" role="alert">
            {error}
          </p>
        ) : null}
        {ran ? (
          <ul className="dx-grid" aria-label={t("chat.diagnosis.title")}>
            {sortCapabilities(report.capabilities).map((item) => (
              <CapabilityCard
                key={item.capability}
                item={item}
                now={now}
                busy={running}
                onRetest={() => void run({ kind: "capability", capability: item.capability })}
                onOpenSettings={onOpenSettings}
              />
            ))}
          </ul>
        ) : (
          <p className="dx-muted dx-empty">{t("chat.diagnosis.empty")}</p>
        )}
        <p className="dx-muted dx-footnote">{t("chat.diagnosis.footnote")}</p>
      </div>
    </section>
  );
}
