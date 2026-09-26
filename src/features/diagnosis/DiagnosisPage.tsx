import { useTranslation } from "react-i18next";
import type { DiagnosisItem, DiagnosisStatus } from "../../lib/generated/diagnosis";
import "./DiagnosisPage.css";
import { useDiagnosisReport } from "./useDiagnosisReport";

const LARM_PROVIDER_ID = "provider.lan-llm-dynamic";
const LARM_SERVICES = [
  { id: "llm", itemId: "harness.llm" },
  { id: "backchannel", itemId: "harness.backchannel" },
  { id: "asr", itemId: "harness.asr" },
  { id: "tts", itemId: "harness.tts" },
  { id: "embedding", itemId: "harness.embedding" },
] as const;
const STATE_ITEMS = [
  { id: "sqlite", titleKey: "sqlite" },
  { id: "memory.personal_state", titleKey: "personalState" },
  { id: "world.status", titleKey: "world" },
  { id: "tool_selection.catalog", titleKey: "toolchain" },
] as const;

const STAGE_CLASS: Record<DiagnosisStatus, string> = {
  ok: "success",
  warn: "waiting",
  fail: "failure",
  running: "running",
  skipped: "waiting",
};

export function DiagnosisPage() {
  const { t, i18n } = useTranslation();
  const { report, running, error, rerun } = useDiagnosisReport();
  const started = report != null;
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const items = report?.items ?? [];
  const mode = items.some((item) => item.id === "diagnosis.mode.operational")
    ? "operational"
    : "fast";
  const services = larmServices(items);
  const state = localState(items);
  const reasoning = items.filter(isReasoningProvider);
  const shown = new Set([
    ...services.map((stage) => stage.item?.id).filter((id): id is string => id != null),
    ...state.map((stage) => stage.item?.id).filter((id): id is string => id != null),
    ...reasoning.map((item) => item.id),
    LARM_PROVIDER_ID,
    "harness.tts",
    "provider.system-tts",
    "diagnosis.mode.fast",
    "diagnosis.mode.operational",
  ]);
  const other = items.filter((item) => !shown.has(item.id) && !isReasoningProvider(item));
  const overall = running ? "running" : (report?.overall ?? "skipped");
  const failed = overall === "fail" || overall === "warn";

  return (
    <section className="diagnosis-page" aria-label={t("navigation.diagnosis")}>
      <div className="diagnosis-page-header" role="toolbar" aria-label={t("navigation.diagnosis")}>
        <button type="button" onClick={() => void rerun("fast")} disabled={running}>
          {t("chat.diagnosis.fast")}
        </button>
        <button type="button" onClick={() => void rerun("operational")} disabled={running}>
          {t("chat.diagnosis.operational")}
        </button>
      </div>
      {!started ? (
        error ? (
          <p className="diagnosis-page-error" role="alert">
            {error}
          </p>
        ) : null
      ) : (
        <div className="diagnosis-page-content">
          <section className="diagnosis-summary" aria-labelledby="diagnosis-page-title">
            <div className="diagnosis-summary-heading">
              <div>
                <p>{t("chat.diagnosis.eyebrow")}</p>
                <h2 id="diagnosis-page-title">{t("chat.diagnosis.title")}</h2>
              </div>
              <span className={`diagnosis-live${running ? "" : " is-settled"}`}>
                {running ? t("chat.diagnosis.rerunning") : t("chat.diagnosis.live")}
              </span>
            </div>
            <p className="diagnosis-meta">
              {t(`chat.diagnosis.mode.${mode}`)} ·{" "}
              {report?.finishedAt
                ? `${t("chat.diagnosis.finished")} ${formatFinished(report.finishedAt, locale)}`
                : t("chat.diagnosis.saved")}
              {report ? <small> · revision {report.revision}</small> : null}
            </p>
            <p className={`diagnosis-verdict diagnosis-verdict-${failed ? "failure" : "normal"}`}>
              {t(
                `chat.diagnosis.verdict.${mode === "fast" && overall === "ok" ? "fastOk" : overall}`,
              )}
            </p>
            <div className="diagnosis-lane">
              <h3>{t("chat.diagnosis.lanes.larm")}</h3>
              <div className="diagnosis-services">
                {services.map((stage) => (
                  <StageCard
                    key={stage.id}
                    title={t(`chat.diagnosis.services.${stage.id}`)}
                    status={stage.status}
                    detail={stage.detail}
                    connected={false}
                  />
                ))}
              </div>
            </div>
            <div className="diagnosis-lane diagnosis-lane-foundation">
              <h3>{t("chat.diagnosis.lanes.state")}</h3>
              <div className="diagnosis-state">
                {state.map((stage) => (
                  <StageCard
                    key={stage.id}
                    title={t(`chat.diagnosis.state.${stage.id}`)}
                    status={stage.status}
                    detail={stage.detail}
                    connected={false}
                  />
                ))}
              </div>
            </div>
            {reasoning.length > 0 ? (
              <div className="diagnosis-lane diagnosis-lane-secondary">
                <h3>{t("chat.diagnosis.lanes.reasoning")}</h3>
                <div className="diagnosis-reasoning">
                  {reasoning.map((item) => (
                    <StageCard
                      key={item.id}
                      title={item.label}
                      status={item.status}
                      detail={plainMessage(item)}
                      connected={false}
                    />
                  ))}
                </div>
              </div>
            ) : null}
          </section>
          {error ? (
            <p className="diagnosis-page-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="diagnosis-table-frame">
            <table className="diagnosis-table">
              <caption>{t("chat.diagnosis.itemsHeading")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t("chat.diagnosis.columns.name")}</th>
                  <th scope="col">{t("chat.diagnosis.columns.status")}</th>
                  <th scope="col">{t("chat.diagnosis.columns.detail")}</th>
                </tr>
              </thead>
              <tbody>
                <ItemRows
                  title={t("chat.diagnosis.lanes.larm")}
                  items={services.flatMap((stage) => (stage.item ? [stage.item] : []))}
                />
                <ItemRows
                  title={t("chat.diagnosis.lanes.state")}
                  items={state.flatMap((stage) => (stage.item ? [stage.item] : []))}
                />
                <ItemRows title={t("chat.diagnosis.lanes.reasoning")} items={reasoning} />
                <ItemRows title={t("chat.diagnosis.lanes.other")} items={other} />
              </tbody>
            </table>
          </div>
        </div>
      )}
    </section>
  );
}

function StageCard({
  title,
  status,
  detail,
  connected,
}: {
  title: string;
  status: DiagnosisStatus;
  detail: string;
  connected: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div className="diagnosis-stage-wrap">
      <article className={`diagnosis-stage diagnosis-stage-${STAGE_CLASS[status]}`}>
        <div className="diagnosis-stage-title">
          <strong>{title}</strong>
          <span>{t(`chat.diagnosis.status.${status}`)}</span>
        </div>
        {detail ? <p>{detail}</p> : null}
      </article>
      {connected ? (
        <span className="diagnosis-arrow" aria-hidden="true">
          →
        </span>
      ) : null}
    </div>
  );
}

function ItemRows({ title, items }: { title: string; items: DiagnosisItem[] }) {
  const { t } = useTranslation();
  if (items.length === 0) return null;
  return (
    <>
      <tr className="diagnosis-table-section">
        <th scope="rowgroup" colSpan={3}>
          {title}
        </th>
      </tr>
      {items.map((item) => (
        <tr key={item.id}>
          <th scope="row">{t(`chat.diagnosis.items.${item.id}`, { defaultValue: item.label })}</th>
          <td>
            <span className={`diagnosis-status diagnosis-status-${item.status}`}>
              {t(`chat.diagnosis.status.${item.status}`)}
            </span>
          </td>
          <td>
            {[plainMessage(item), item.latencyMs != null ? `${item.latencyMs} ms` : ""]
              .filter(Boolean)
              .join(" · ")}
          </td>
        </tr>
      ))}
    </>
  );
}

function larmServices(items: DiagnosisItem[]) {
  return LARM_SERVICES.flatMap((service) => {
    const found = items.find((item) => item.id === service.itemId);
    return found ? [card(service.id, found)] : [];
  });
}

function localState(items: DiagnosisItem[]) {
  return STATE_ITEMS.flatMap((entry) => {
    const found = items.find((item) => item.id === entry.id);
    return found ? [card(entry.titleKey, found)] : [];
  });
}

function card(id: string, item: DiagnosisItem | undefined) {
  return {
    id,
    status: item?.status ?? "skipped",
    detail: item ? plainMessage(item) : "",
    item,
  } satisfies {
    id: string;
    status: DiagnosisStatus;
    detail: string;
    item: DiagnosisItem | undefined;
  };
}

function isReasoningProvider(item: DiagnosisItem) {
  return item.group === "llm" && item.id.startsWith("provider.") && item.id !== LARM_PROVIDER_ID;
}

function plainMessage(item: DiagnosisItem) {
  const prefix = `${item.label}: `;
  return item.message.startsWith(prefix) ? item.message.slice(prefix.length) : item.message;
}

function formatFinished(value: string, locale: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "medium" }).format(date);
}
