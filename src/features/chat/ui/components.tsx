import { useState } from "react";
import { useTranslation } from "react-i18next";
import { uiApi, notifyUiHistoryChanged } from "./api";
import { useUiContext, useUiData, useUiField } from "./context";
import { uiQueries } from "./queryCache";

function DataStatus({
  loading,
  error,
  capturedAt,
}: {
  loading: boolean;
  error?: string;
  capturedAt?: string;
}) {
  const { t, i18n } = useTranslation();
  return (
    <p className="ui-data-status" role={error ? "status" : undefined}>
      {error
        ? t("genui.unavailable")
        : loading
          ? t("genui.loading")
          : capturedAt
            ? new Date(Number(capturedAt)).toLocaleString(i18n.language)
            : ""}
    </p>
  );
}
export function MetricView({
  source,
  field,
  label,
}: {
  source: string;
  field: string;
  label: string;
}) {
  const { t } = useTranslation();
  const result = useUiData(source);
  return (
    <section className="ui-metric">
      <span>{label}</span>
      <strong>{String(result.data?.rows[0]?.[field] ?? t("genui.missing"))}</strong>
      <DataStatus {...result} capturedAt={result.data?.capturedAt} />
    </section>
  );
}
export function TableView({
  source,
  columns,
  stateId,
}: {
  source: string;
  columns: string;
  stateId: string;
}) {
  const { t, i18n } = useTranslation();
  const result = useUiData(source);
  const [filter, setFilter] = useUiField(`${stateId}:filter`, "");
  const [sort, setSort] = useUiField(`${stateId}:sort`, "");
  const [descending, setDescending] = useUiField(`${stateId}:descending`, false);
  const [page, setPage] = useUiField(`${stateId}:page`, 0);
  const fields = columns
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const rows = [...(result.data?.rows ?? [])].filter((row) =>
    Object.values(row).some((value) =>
      String(value ?? "")
        .toLocaleLowerCase()
        .includes(String(filter).toLocaleLowerCase()),
    ),
  );
  rows.sort((a, b) => {
    const x = a[String(sort)],
      y = b[String(sort)];
    const compare =
      typeof x === "number" && typeof y === "number"
        ? x - y
        : String(x ?? "").localeCompare(String(y ?? ""), i18n.language, { numeric: true });
    return descending ? -compare : compare;
  });
  const current = Math.max(0, Math.min(Number(page), Math.ceil(rows.length / 10) - 1));
  function cell(field: string, value: string | number | null) {
    if (value === null || value === undefined) return t("genui.missing");
    if (field.endsWith("At")) return new Date(Number(value)).toLocaleString(i18n.language);
    return String(value);
  }
  return (
    <section className="ui-table">
      <label className="ui-filter">
        {t("genui.filter")}
        <input
          maxLength={1000}
          value={String(filter)}
          onChange={(event) => {
            setFilter(event.target.value);
            setPage(0);
          }}
        />
      </label>
      <div className="ui-table-scroll">
        <table>
          <caption>{source === "runtime.history" ? t("genui.history") : t("genui.scope")}</caption>
          <thead>
            <tr>
              {fields.map((field) => (
                <th
                  key={field}
                  scope="col"
                  aria-sort={sort === field ? (descending ? "descending" : "ascending") : "none"}
                >
                  <button
                    onClick={() => {
                      setSort(field);
                      setDescending(sort === field ? !descending : false);
                    }}
                  >
                    {t(`genui.${field}`, { defaultValue: field })}
                  </button>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.slice(current * 10, current * 10 + 10).map((row, index) => (
              <tr key={String(row.id ?? index)}>
                {fields.map((field) => (
                  <td key={field}>{cell(field, row[field])}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!rows.length && !result.loading && <p>{t("genui.empty")}</p>}
      <div className="ui-pagination">
        <button disabled={current === 0} onClick={() => setPage(current - 1)}>
          {t("genui.previous")}
        </button>
        <span>
          {current + 1} / {Math.max(1, Math.ceil(rows.length / 10))}
        </span>
        <button disabled={(current + 1) * 10 >= rows.length} onClick={() => setPage(current + 1)}>
          {t("genui.next")}
        </button>
      </div>
      <DataStatus {...result} capturedAt={result.data?.capturedAt} />
    </section>
  );
}
export function ChartView({ source }: { source: string }) {
  const { t, i18n } = useTranslation();
  const result = useUiData(source);
  const rows = result.data?.rows ?? [];
  const max = Math.max(1, ...rows.map((row) => Number(row.count)));
  const start = Number(rows[0]?.time ?? 0);
  const end = Number(rows[rows.length - 1]?.time ?? start);
  const x = (time: string | number | null) =>
    20 + ((Number(time) - start) / Math.max(1, end - start)) * 560;
  const points = rows
    .map((row) => `${x(row.time)},${160 - (Number(row.count) / max) * 140}`)
    .join(" ");
  return (
    <figure className="ui-chart">
      <figcaption>{t("genui.history")}</figcaption>
      {rows.length ? (
        <>
          <svg viewBox="0 0 600 190" role="img" aria-label={t("genui.history")}>
            <line x1="20" x2="580" y1="160" y2="160" stroke="currentColor" opacity=".4" />
            <polyline fill="none" stroke="currentColor" strokeWidth="3" points={points} />
            {rows.map((row, index) => (
              <circle key={index} cx={x(row.time)} cy={160 - (Number(row.count) / max) * 140} r="3">
                <title>
                  {new Date(Number(row.time)).toLocaleString(i18n.language)}: {String(row.count)}
                </title>
              </circle>
            ))}
            <text x="20" y="185">
              0
            </text>
            <text x="20" y="14">
              {max}
            </text>
          </svg>
          <small>
            {new Date(Number(rows[0].time)).toLocaleString(i18n.language)} —{" "}
            {new Date(Number(rows[rows.length - 1].time)).toLocaleString(i18n.language)}
          </small>
        </>
      ) : (
        <p>{t("genui.empty")}</p>
      )}
      <DataStatus {...result} capturedAt={result.data?.capturedAt} />
    </figure>
  );
}
function CancelRuns() {
  const { instance, conversationId, active, enabled } = useUiContext();
  const { t } = useTranslation();
  const result = useUiData("runtime.runs");
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState("");
  async function cancel(id: string) {
    setPending(true);
    setOutcome("");
    try {
      await uiApi.cancel(instance.id, id, `ui_${crypto.randomUUID()}`);
      setOutcome("genui.cancelDone");
      notifyUiHistoryChanged(conversationId);
    } catch {
      setOutcome("genui.operationFailed");
    } finally {
      setPending(false);
      void uiQueries.refresh(`${conversationId}:runtime.runs`);
    }
  }
  return (
    <div className="ui-actions">
      {result.data?.rows
        .filter((row) => row.status === "running")
        .map((row) => (
          <button
            key={String(row.id)}
            disabled={pending || !active || !enabled}
            onClick={() => void cancel(String(row.id))}
          >
            {t("genui.cancel")} · {String(row.provider ?? row.id)}
          </button>
        ))}
      {!result.loading &&
        !result.error &&
        !result.data?.rows.some((row) => row.status === "running") && <p>{t("genui.empty")}</p>}
      <DataStatus {...result} capturedAt={result.data?.capturedAt} />
      {outcome && <p role="status">{t(outcome)}</p>}
    </div>
  );
}
export function ActionsView({ action }: { action: string }) {
  const { t } = useTranslation();
  const { instance, conversationId, active, enabled } = useUiContext();
  if (instance.mode === "snapshot") return null;
  if (action === "cancel_run") return <CancelRuns />;
  function refresh() {
    const sources = new Set<string>();
    function visit(node: typeof instance.node) {
      if (["Metric", "Status", "Table", "Chart", "ModelStatus"].includes(node.kind))
        sources.add(node.args[0]);
      if (node.kind === "Actions" && node.args[0] === "cancel_run") sources.add("runtime.runs");
      node.children.forEach(visit);
    }
    visit(instance.node);
    sources.forEach((source) => void uiQueries.refresh(`${conversationId}:${source}`));
  }
  return (
    <button disabled={!active || !enabled} onClick={refresh}>
      {t("genui.refresh")}
    </button>
  );
}
