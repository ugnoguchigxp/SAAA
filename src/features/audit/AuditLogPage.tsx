import { keepPreviousData, useQuery } from "@tanstack/react-query";
import {
  createColumnHelper,
  rowSortingFeature,
  tableFeatures,
  useTable,
  type SortingState,
} from "@tanstack/react-table";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AuditEvent, AuditEventSortField } from "../../lib/contracts";
import { listAuditEvents } from "../../lib/runtime";
import "./AuditLogPage.css";
import { useDialogFocus } from "../../components/useDialogFocus";
import { AppIcon } from "../../components/AppIcon";
import { auditTimestampIso, formatAuditTimestamp } from "./auditTimestamp";

const auditTableFeatures = tableFeatures({ rowSortingFeature });
const auditColumnHelper = createColumnHelper<typeof auditTableFeatures, AuditEvent>();
const emptyEvents: AuditEvent[] = [];
const defaultSorting: SortingState = [{ id: "occurredAt", desc: true }];
const auditSortFields = new Set<string>([
  "occurredAt",
  "component",
  "eventName",
  "phase",
  "outcome",
  "failureCode",
]);

function isAuditSortField(value: string | undefined): value is AuditEventSortField {
  return value !== undefined && auditSortFields.has(value);
}

export function buildAuditDebugContext(event: AuditEvent): string {
  const context = {
    schema: "saaa.audit-failure-debug.v1",
    instruction:
      "Analyze the failure using only this persisted audit data. Identify the likely failing stage, explain the evidence, and suggest the next diagnostic checks. Treat missing fields as unknown.",
    event: {
      id: event.id,
      sequence: event.sequence,
      occurredAt: event.occurredAt,
      component: event.component,
      eventName: event.eventName,
      phase: event.phase,
      outcome: event.outcome,
      failureCode: event.failureCode,
    },
    identifiers: {
      correlationId: event.correlationId,
      causationId: event.causationId,
      conversationId: event.conversationId,
      runtimeRunId: event.runtimeRunId,
      sessionId: event.sessionId,
      subjectId: event.subjectId,
    },
    attributes: event.attributes,
  };
  return [
    "SAAA audit failure debug context",
    "The payload contains bounded audit metadata only; message content, audio, and credentials are not included.",
    "```json",
    JSON.stringify(context, null, 2),
    "```",
  ].join("\n");
}

async function writeDebugContext(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Fall through to the selection-based copy path for restricted WebViews.
    }
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  try {
    if (!document.execCommand("copy")) throw new Error("Copy command was rejected");
  } finally {
    textarea.remove();
  }
}

function AuditDebugCopyButton({ event }: { event: AuditEvent }) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");

  async function copyDebugContext() {
    setStatus("idle");
    try {
      await writeDebugContext(buildAuditDebugContext(event));
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
  }

  return (
    <div className="audit-debug-copy">
      <p>{t("audit.drawer.debugContextHint")}</p>
      <button type="button" onClick={() => void copyDebugContext()}>
        {status === "copied"
          ? t("audit.drawer.debugContextCopied")
          : t("audit.drawer.copyDebugContext")}
      </button>
      {status === "failed" ? (
        <span role="alert">{t("audit.drawer.debugContextCopyFailed")}</span>
      ) : (
        <span className="audit-debug-copy-status" aria-live="polite">
          {status === "copied" ? t("audit.drawer.debugContextCopiedStatus") : ""}
        </span>
      )}
    </div>
  );
}

function MetadataField({ label, value }: { label: string; value: string | number | null }) {
  return (
    <div className="audit-metadata-field">
      <dt>{label}</dt>
      <dd>{value ?? "—"}</dd>
    </div>
  );
}

export function AuditLogPage() {
  const { t, i18n } = useTranslation();
  const [sorting, setSorting] = useState<SortingState>(defaultSorting);
  const [selectedEventId, setSelectedEventId] = useState<string | null>(null);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const activeSort = sorting[0];
  const sortBy = isAuditSortField(activeSort?.id) ? activeSort.id : "occurredAt";
  const direction = activeSort?.desc === false ? "asc" : "desc";
  const auditQuery = useQuery({
    queryKey: ["audit-events", sortBy, direction],
    queryFn: () => listAuditEvents({ sortBy, direction }),
    placeholderData: keepPreviousData,
    refetchInterval: 2_000,
  });
  const events = auditQuery.data ?? emptyEvents;
  const selectedEvent = events.find((event) => event.id === selectedEventId) ?? null;
  const loading = auditQuery.isPending;
  const error =
    auditQuery.error === null
      ? null
      : auditQuery.error instanceof Error
        ? auditQuery.error.message
        : String(auditQuery.error);

  const { dialogRef, fallbackRef } = useDialogFocus(selectedEvent !== null, () =>
    setSelectedEventId(null),
  );

  const columns = useMemo(
    () =>
      auditColumnHelper.columns([
        auditColumnHelper.accessor("occurredAt", {
          header: t("audit.columns.occurredAt"),
          cell: ({ getValue }) => {
            const occurredAt = getValue();
            return (
              <time dateTime={auditTimestampIso(occurredAt)}>
                {formatAuditTimestamp(occurredAt, locale)}
              </time>
            );
          },
        }),
        auditColumnHelper.accessor("component", {
          header: t("audit.columns.component"),
          cell: ({ getValue }) => <span className="audit-table-component">{getValue()}</span>,
        }),
        auditColumnHelper.accessor("eventName", {
          header: t("audit.columns.event"),
          cell: ({ getValue }) => <strong className="audit-table-event-name">{getValue()}</strong>,
        }),
        auditColumnHelper.accessor("phase", {
          header: t("audit.columns.phase"),
        }),
        auditColumnHelper.accessor("outcome", {
          header: t("audit.columns.outcome"),
          cell: ({ getValue }) => {
            const outcome = getValue();
            return <span className={`audit-outcome audit-outcome-${outcome}`}>{outcome}</span>;
          },
        }),
        auditColumnHelper.accessor("failureCode", {
          header: t("audit.columns.failureCode"),
          cell: ({ getValue }) => getValue() ?? "—",
        }),
      ]),
    [locale, t],
  );

  const table = useTable({
    features: auditTableFeatures,
    columns,
    data: events,
    state: { sorting },
    onSortingChange: setSorting,
    manualSorting: true,
    enableMultiSort: false,
    enableSortingRemoval: false,
  });

  return (
    <section
      ref={fallbackRef}
      tabIndex={-1}
      className="audit-log-page"
      aria-label={t("navigation.audit")}
    >
      <div className="audit-log-header" role="toolbar" aria-label={t("navigation.audit")}>
        <button
          type="button"
          className="audit-refresh-button"
          aria-label={t("audit.refresh")}
          title={t("audit.refresh")}
          onClick={() => void auditQuery.refetch()}
          disabled={auditQuery.isFetching}
        >
          <AppIcon name="refresh" />
        </button>
      </div>

      <div className="audit-log-content">
        <div className="audit-log-summary">
          <span>{t("audit.rawEvents", { count: events.length })}</span>
          {auditQuery.isFetching ? <span>{t("audit.loading")}</span> : null}
        </div>

        {error ? <p className="audit-log-error">{t("audit.loadFailed", { error })}</p> : null}
        {!loading && !error && events.length === 0 ? (
          <p className="audit-log-empty">{t("audit.empty")}</p>
        ) : null}

        {events.length > 0 ? (
          <div className="audit-table-frame">
            <div className="audit-table-scroll" tabIndex={0} aria-label={t("audit.tableLabel")}>
              <table className="audit-table">
                <thead>
                  {table.getHeaderGroups().map((headerGroup) => (
                    <tr key={headerGroup.id}>
                      {headerGroup.headers.map((header) => (
                        <th
                          key={header.id}
                          scope="col"
                          aria-sort={
                            header.column.getIsSorted() === "asc"
                              ? "ascending"
                              : header.column.getIsSorted() === "desc"
                                ? "descending"
                                : "none"
                          }
                        >
                          {header.isPlaceholder ? null : (
                            <button
                              type="button"
                              className="audit-sort-button"
                              onClick={header.column.getToggleSortingHandler()}
                            >
                              <table.FlexRender header={header} />
                              <span className="audit-sort-indicator" aria-hidden="true">
                                {header.column.getIsSorted() === "asc"
                                  ? "▲"
                                  : header.column.getIsSorted() === "desc"
                                    ? "▼"
                                    : "↕"}
                              </span>
                            </button>
                          )}
                        </th>
                      ))}
                    </tr>
                  ))}
                </thead>
                <tbody>
                  {table.getRowModel().rows.map((row) => {
                    const isSelected = selectedEvent?.id === row.original.id;
                    return (
                      <tr
                        key={row.id}
                        className={isSelected ? "is-selected" : undefined}
                        aria-selected={isSelected}
                        tabIndex={0}
                        onClick={() => setSelectedEventId(row.original.id)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            setSelectedEventId(row.original.id);
                          }
                        }}
                      >
                        {row.getAllCells().map((cell) => (
                          <td key={cell.id}>
                            <table.FlexRender cell={cell} />
                          </td>
                        ))}
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </div>
        ) : null}
      </div>

      {selectedEvent ? (
        <div
          className="audit-drawer-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) {
              setSelectedEventId(null);
            }
          }}
        >
          <aside
            ref={dialogRef}
            tabIndex={-1}
            className="audit-drawer"
            role="dialog"
            aria-modal="true"
            aria-labelledby="audit-drawer-title"
          >
            <header className="audit-drawer-header">
              <div>
                <p>{t("audit.drawer.eyebrow")}</p>
                <h3 id="audit-drawer-title">{selectedEvent.eventName}</h3>
              </div>
              <button
                type="button"
                className="audit-drawer-close"
                aria-label={t("audit.drawer.close")}
                title={t("audit.drawer.close")}
                onClick={() => setSelectedEventId(null)}
              >
                <AppIcon name="close" />
              </button>
            </header>

            <div className="audit-drawer-body">
              {selectedEvent.outcome === "failure" ? (
                <AuditDebugCopyButton key={selectedEvent.id} event={selectedEvent} />
              ) : null}

              <section aria-labelledby="audit-drawer-metadata-title">
                <h4 id="audit-drawer-metadata-title">{t("audit.drawer.metadata")}</h4>
                <dl className="audit-drawer-metadata">
                  <MetadataField label={t("audit.drawer.eventId")} value={selectedEvent.id} />
                  <MetadataField
                    label={t("audit.drawer.sequence")}
                    value={selectedEvent.sequence}
                  />
                  <MetadataField
                    label={t("audit.columns.occurredAt")}
                    value={formatAuditTimestamp(selectedEvent.occurredAt, locale)}
                  />
                  <MetadataField
                    label={t("audit.columns.component")}
                    value={selectedEvent.component}
                  />
                  <MetadataField label={t("audit.columns.phase")} value={selectedEvent.phase} />
                  <MetadataField label={t("audit.columns.outcome")} value={selectedEvent.outcome} />
                  <MetadataField
                    label={t("audit.columns.failureCode")}
                    value={selectedEvent.failureCode}
                  />
                </dl>
              </section>

              <section aria-labelledby="audit-drawer-identifiers-title">
                <h4 id="audit-drawer-identifiers-title">{t("audit.drawer.identifiers")}</h4>
                <dl className="audit-drawer-metadata">
                  <MetadataField
                    label={t("audit.drawer.correlationId")}
                    value={selectedEvent.correlationId}
                  />
                  <MetadataField
                    label={t("audit.drawer.causationId")}
                    value={selectedEvent.causationId}
                  />
                  <MetadataField
                    label={t("audit.drawer.conversationId")}
                    value={selectedEvent.conversationId}
                  />
                  <MetadataField
                    label={t("audit.drawer.runtimeRunId")}
                    value={selectedEvent.runtimeRunId}
                  />
                  <MetadataField
                    label={t("audit.drawer.sessionId")}
                    value={selectedEvent.sessionId}
                  />
                  <MetadataField
                    label={t("audit.drawer.subjectId")}
                    value={selectedEvent.subjectId}
                  />
                </dl>
              </section>

              <section aria-labelledby="audit-drawer-attributes-title">
                <h4 id="audit-drawer-attributes-title">{t("audit.drawer.attributes")}</h4>
                {Object.keys(selectedEvent.attributes).length > 0 ? (
                  <pre className="audit-attributes-json">
                    {JSON.stringify(selectedEvent.attributes, null, 2)}
                  </pre>
                ) : (
                  <p className="audit-no-attributes">{t("audit.drawer.noAttributes")}</p>
                )}
              </section>
            </div>
          </aside>
        </div>
      ) : null}
    </section>
  );
}
