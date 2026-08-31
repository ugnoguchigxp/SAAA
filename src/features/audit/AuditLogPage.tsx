import { createColumnHelper, tableFeatures, useTable } from "@tanstack/react-table";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AuditEvent } from "../../lib/contracts";
import { listAuditEvents } from "../../lib/runtime";
import "./AuditLogPage.css";

const auditTableFeatures = tableFeatures({});
const auditColumnHelper = createColumnHelper<typeof auditTableFeatures, AuditEvent>();

function occurredAtIso(value: AuditEvent["occurredAt"]) {
  return new Date(value).toISOString();
}

function formatOccurredAt(value: AuditEvent["occurredAt"], locale: string) {
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(occurredAtIso(value)));
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
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const [selectedEvent, setSelectedEvent] = useState<AuditEvent | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const locale = i18n.resolvedLanguage ?? i18n.language;

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const nextEvents = await listAuditEvents();
      setEvents(nextEvents);
      setSelectedEvent((current) =>
        current ? (nextEvents.find((event) => event.id === current.id) ?? null) : null,
      );
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!selectedEvent) {
      return undefined;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setSelectedEvent(null);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [selectedEvent]);

  const columns = useMemo(
    () =>
      auditColumnHelper.columns([
        auditColumnHelper.accessor("occurredAt", {
          header: t("audit.columns.occurredAt"),
          cell: ({ getValue }) => {
            const occurredAt = getValue();
            return <time dateTime={occurredAtIso(occurredAt)}>{formatOccurredAt(occurredAt, locale)}</time>;
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
  });

  return (
    <section className="audit-log-page" aria-labelledby="audit-log-title">
      <header className="audit-log-header">
        <div>
          <p>{t("audit.eyebrow")}</p>
          <h2 id="audit-log-title">{t("audit.title")}</h2>
          <span>{t("audit.description")}</span>
        </div>
        <button type="button" className="audit-refresh-button" onClick={() => void load()} disabled={loading}>
          {t("audit.refresh")}
        </button>
      </header>

      <div className="audit-log-content">
        <div className="audit-log-summary">
          <span>{t("audit.latestCount", { count: events.length })}</span>
          {loading ? <span>{t("audit.loading")}</span> : null}
        </div>

        {error ? <p className="audit-log-error">{t("audit.loadFailed", { error })}</p> : null}
        {!loading && !error && events.length === 0 ? <p className="audit-log-empty">{t("audit.empty")}</p> : null}

        {events.length > 0 ? (
          <div className="audit-table-frame">
            <div className="audit-table-scroll" tabIndex={0} aria-label={t("audit.tableLabel")}>
              <table className="audit-table">
                <thead>
                  {table.getHeaderGroups().map((headerGroup) => (
                    <tr key={headerGroup.id}>
                      {headerGroup.headers.map((header) => (
                        <th key={header.id} scope="col">
                          {header.isPlaceholder ? null : <table.FlexRender header={header} />}
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
                        onClick={() => setSelectedEvent(row.original)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            setSelectedEvent(row.original);
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
              setSelectedEvent(null);
            }
          }}
        >
          <aside className="audit-drawer" role="dialog" aria-modal="true" aria-labelledby="audit-drawer-title">
            <header className="audit-drawer-header">
              <div>
                <p>{t("audit.drawer.eyebrow")}</p>
                <h3 id="audit-drawer-title">{selectedEvent.eventName}</h3>
              </div>
              <button
                type="button"
                className="audit-drawer-close"
                aria-label={t("audit.drawer.close")}
                onClick={() => setSelectedEvent(null)}
              >
                ×
              </button>
            </header>

            <div className="audit-drawer-body">
              <section aria-labelledby="audit-drawer-metadata-title">
                <h4 id="audit-drawer-metadata-title">{t("audit.drawer.metadata")}</h4>
                <dl className="audit-drawer-metadata">
                  <MetadataField label={t("audit.drawer.eventId")} value={selectedEvent.id} />
                  <MetadataField label={t("audit.drawer.sequence")} value={selectedEvent.sequence} />
                  <MetadataField
                    label={t("audit.columns.occurredAt")}
                    value={formatOccurredAt(selectedEvent.occurredAt, locale)}
                  />
                  <MetadataField label={t("audit.columns.component")} value={selectedEvent.component} />
                  <MetadataField label={t("audit.columns.phase")} value={selectedEvent.phase} />
                  <MetadataField label={t("audit.columns.outcome")} value={selectedEvent.outcome} />
                  <MetadataField label={t("audit.columns.failureCode")} value={selectedEvent.failureCode} />
                </dl>
              </section>

              <section aria-labelledby="audit-drawer-identifiers-title">
                <h4 id="audit-drawer-identifiers-title">{t("audit.drawer.identifiers")}</h4>
                <dl className="audit-drawer-metadata">
                  <MetadataField label={t("audit.drawer.correlationId")} value={selectedEvent.correlationId} />
                  <MetadataField label={t("audit.drawer.causationId")} value={selectedEvent.causationId} />
                  <MetadataField label={t("audit.drawer.conversationId")} value={selectedEvent.conversationId} />
                  <MetadataField label={t("audit.drawer.runtimeRunId")} value={selectedEvent.runtimeRunId} />
                  <MetadataField label={t("audit.drawer.sessionId")} value={selectedEvent.sessionId} />
                  <MetadataField label={t("audit.drawer.subjectId")} value={selectedEvent.subjectId} />
                </dl>
              </section>

              <section aria-labelledby="audit-drawer-attributes-title">
                <h4 id="audit-drawer-attributes-title">{t("audit.drawer.attributes")}</h4>
                {Object.keys(selectedEvent.attributes).length > 0 ? (
                  <pre className="audit-attributes-json">{JSON.stringify(selectedEvent.attributes, null, 2)}</pre>
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
