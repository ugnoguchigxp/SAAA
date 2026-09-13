import "./ui.css";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { uiApi, notifyUiHistoryChanged, type SavedView } from "./api";
import { useGenUiEnabled, setGenUiEnabled } from "./settings";
export function UiControls({ conversationId }: { conversationId?: string }) {
  const { t } = useTranslation();
  const enabled = useGenUiEnabled();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [views, setViews] = useState<SavedView[]>([]);
  const searchGeneration = useRef(0);
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!open) return;
    let disposed = false;
    const generation = ++searchGeneration.current;
    const timer = setTimeout(() => {
      void uiApi
        .search(query)
        .then((value) => {
          if (!disposed && generation === searchGeneration.current) {
            setViews(value);
            setError(false);
          }
        })
        .catch(() => {
          if (!disposed && generation === searchGeneration.current) setError(true);
        });
    }, 150);
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [open, query]);
  async function openView(id: string) {
    if (!conversationId) return;
    setBusy(true);
    setError(false);
    try {
      await uiApi.open(conversationId, id);
      notifyUiHistoryChanged(conversationId);
      setOpen(false);
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  }
  async function archiveView(id: string) {
    setBusy(true);
    setError(false);
    try {
      await uiApi.archive(id);
      searchGeneration.current++;
      setViews((current) => current.filter((view) => view.id !== id));
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="ui-controls">
      <label>
        <input
          type="checkbox"
          checked={enabled}
          disabled={busy}
          onChange={(event) => {
            setBusy(true);
            setError(false);
            void setGenUiEnabled(event.target.checked)
              .catch(() => setError(true))
              .finally(() => setBusy(false));
          }}
        />
        {t("genui.enable")}
      </label>
      <button type="button" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
        {t("genui.saved")}
      </button>
      {enabled && <small>{t("genui.compatibility")}</small>}
      {open && (
        <section className="ui-saved">
          <label>
            {t("genui.search")}
            <input
              value={query}
              maxLength={256}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
          {views.map((view) => (
            <div key={view.id}>
              <span>
                {view.name} · v{view.revision}
              </span>
              <button
                disabled={!enabled || busy || !conversationId}
                onClick={() => void openView(view.id)}
              >
                {t("genui.open")}
              </button>
              <button disabled={!enabled || busy} onClick={() => void archiveView(view.id)}>
                {t("genui.archive")}
              </button>
            </div>
          ))}
          {!views.length && <p>{t("genui.empty")}</p>}
          <button onClick={() => setOpen(false)}>{t("genui.close")}</button>
        </section>
      )}
      {error && <span role="alert">{t("genui.operationFailed")}</span>}
    </div>
  );
}
