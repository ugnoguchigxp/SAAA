import {
  createColumnHelper,
  rowSortingFeature,
  tableFeatures,
  useTable,
  type SortingState,
} from "@tanstack/react-table";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import {
  personalStateApi,
  type PersonalSourcePage,
  type PersonalStateItem,
  type PersonalStateSnapshot,
} from "./api";
import "../workspacePages.css";

const memoryTableFeatures = tableFeatures({ rowSortingFeature });
const columnHelper = createColumnHelper<typeof memoryTableFeatures, PersonalStateItem>();

function displayValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "—";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function MemoryPage({
  onOpenRecord,
  onCorrect,
}: {
  onOpenRecord: (sourceId: string) => void;
  onCorrect: (item: PersonalStateItem) => void;
}) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<PersonalStateSnapshot | null>(null);
  const [sourcePage, setSourcePage] = useState<PersonalSourcePage | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sorting, setSorting] = useState<SortingState>([{ id: "kind", desc: false }]);
  const [loading, setLoading] = useState(true);
  const [busySource, setBusySource] = useState<string | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [nextSnapshot, nextSources] = await Promise.all([
        personalStateApi.snapshot(),
        personalStateApi.sources(),
      ]);
      setSnapshot(nextSnapshot);
      setSourcePage(nextSources);
      setSelectedId((current) =>
        current && nextSnapshot.items.some((item) => item.id === current)
          ? current
          : (nextSnapshot.items[0]?.id ?? null),
      );
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const columns = useMemo(
    () =>
      columnHelper.columns([
        columnHelper.accessor((item) => displayValue(item.value), {
          id: "content",
          header: t("memoryPage.columns.content"),
          cell: ({ getValue }) => <span className="memory-content-cell">{getValue()}</span>,
        }),
        columnHelper.accessor("key", {
          id: "kind",
          header: t("memoryPage.columns.kind"),
        }),
        columnHelper.accessor("status", {
          header: t("memoryPage.columns.status"),
          cell: ({ getValue }) => <span className="state-chip">{getValue()}</span>,
        }),
        columnHelper.accessor((item) => item.source.length, {
          id: "sources",
          header: t("memoryPage.columns.sources"),
        }),
      ]),
    [t],
  );

  const table = useTable({
    features: memoryTableFeatures,
    columns,
    data: snapshot?.items ?? [],
    state: { sorting },
    onSortingChange: setSorting,
    enableMultiSort: false,
  });
  const selected = snapshot?.items.find((item) => item.id === selectedId) ?? null;
  const sourceById = useMemo(
    () => new Map((sourcePage?.sources ?? []).map((source) => [source.id, source])),
    [sourcePage],
  );

  async function forget(sourceId: string) {
    if (!window.confirm(t("memoryPage.forgetConfirm"))) return;
    setBusySource(sourceId);
    try {
      const next = await personalStateApi.forget(sourceId);
      setSnapshot(next);
      setSourcePage(await personalStateApi.sources());
      setSelectedId((current) =>
        current && next.items.some((item) => item.id === current)
          ? current
          : (next.items[0]?.id ?? null),
      );
      setError("");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusySource(null);
    }
  }

  async function loadMoreSources() {
    if (!sourcePage?.hasMore) return;
    try {
      const next = await personalStateApi.sources(sourcePage.nextSequence);
      setSourcePage({
        sources: [...sourcePage.sources, ...next.sources],
        nextSequence: next.nextSequence,
        hasMore: next.hasMore,
      });
    } catch (cause) {
      setError(String(cause));
    }
  }

  return (
    <section className="workspace-page memory-page" aria-label={t("navigation.memory")}>
      <div
        className="workspace-page-header workspace-page-toolbar"
        role="toolbar"
        aria-label={t("navigation.memory")}
      >
        <button
          type="button"
          className="workspace-secondary-button workspace-symbol-button"
          aria-label={t("common.refresh")}
          title={t("common.refresh")}
          onClick={() => void refresh()}
        >
          <AppIcon name="refresh" />
        </button>
      </div>
      <div className="memory-layout">
        <div className="memory-main">
          {snapshot &&
          (!snapshot.enabled || !snapshot.contractReady || snapshot.pendingCount > 0) ? (
            <div className="workspace-notices" role="status">
              {!snapshot.enabled ? <span>{t("memoryPage.disabled")}</span> : null}
              {!snapshot.contractReady ? <span>{t("memoryPage.contractPending")}</span> : null}
              {snapshot.pendingCount > 0 ? (
                <span>{t("memoryPage.pending", { count: snapshot.pendingCount })}</span>
              ) : null}
            </div>
          ) : null}
          {error ? (
            <p className="workspace-error" role="alert">
              {error}
            </p>
          ) : null}
          {loading ? <p className="workspace-empty">{t("common.loading")}</p> : null}
          {!loading && snapshot?.items.length === 0 ? (
            <p className="workspace-empty">{t("memoryPage.empty")}</p>
          ) : null}
          {snapshot && snapshot.items.length > 0 ? (
            <div className="workspace-table-scroll" tabIndex={0}>
              <table className="workspace-table memory-table">
                <thead>
                  {table.getHeaderGroups().map((group) => (
                    <tr key={group.id}>
                      {group.headers.map((header) => (
                        <th key={header.id} scope="col">
                          {header.isPlaceholder ? null : (
                            <button
                              type="button"
                              className="workspace-sort-button"
                              onClick={header.column.getToggleSortingHandler()}
                            >
                              <table.FlexRender header={header} />
                              <span aria-hidden="true">
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
                  {table.getRowModel().rows.map((row) => (
                    <tr
                      key={row.id}
                      tabIndex={0}
                      className={row.original.id === selectedId ? "is-selected" : undefined}
                      aria-selected={row.original.id === selectedId}
                      onClick={() => setSelectedId(row.original.id)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          setSelectedId(row.original.id);
                        }
                      }}
                    >
                      {row.getAllCells().map((cell) => (
                        <td key={cell.id}>
                          <table.FlexRender cell={cell} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
        </div>
        <aside className="memory-detail" aria-label={t("memoryPage.detailLabel")}>
          {selected ? (
            <>
              <div className="memory-detail-heading">
                <span className="state-chip">{selected.status}</span>
                <strong>{selected.key}</strong>
              </div>
              <pre>{displayValue(selected.value)}</pre>
              <button
                type="button"
                className="workspace-secondary-button"
                onClick={() => onCorrect(selected)}
              >
                {t("memoryPage.correct")}
              </button>
              <h2>{t("memoryPage.sources")}</h2>
              {selected.source.length === 0 ? <p>{t("memoryPage.noSources")}</p> : null}
              <div className="memory-sources">
                {selected.source.map(({ id }) => {
                  const source = sourceById.get(id);
                  return (
                    <article key={id}>
                      <p>{source?.preview ?? id}</p>
                      <div>
                        <button
                          type="button"
                          className="workspace-text-button"
                          onClick={() => onOpenRecord(id)}
                        >
                          {t("memoryPage.openRecord")}
                        </button>
                        <button
                          type="button"
                          className="workspace-danger-button"
                          disabled={busySource === id}
                          onClick={() => void forget(id)}
                        >
                          {t("memoryPage.forget")}
                        </button>
                      </div>
                    </article>
                  );
                })}
              </div>
              {sourcePage?.hasMore ? (
                <button
                  type="button"
                  className="workspace-secondary-button"
                  onClick={() => void loadMoreSources()}
                >
                  {t("common.loadMore")}
                </button>
              ) : null}
            </>
          ) : (
            <p className="workspace-empty">{t("memoryPage.select")}</p>
          )}
        </aside>
      </div>
    </section>
  );
}
