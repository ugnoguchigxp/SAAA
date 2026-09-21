import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import type { UiInstance, UiViewRevision } from "../../../lib/generated/generativeUi";
import { uiApi } from "../ui/api";
import { UiContext } from "../ui/context";
import { uiStates } from "../ui/instanceState";
import { useGenUiEnabled } from "../ui/settings";
import { SemanticRenderer } from "../ui/SemanticRenderer";
import { UiBoundary } from "../ui/UiBoundary";
import { artifactWidthFor } from "./artifactWidth";
import { lineDiff } from "./lineDiff";

type ArtifactTab = {
  conversationId: string;
  instance: UiInstance;
};
type ArtifactWorkspaceContextValue = {
  open: (instance: UiInstance, conversationId: string) => void;
};
const ArtifactWorkspaceContext = createContext<ArtifactWorkspaceContextValue | null>(null);

export function useArtifactWorkspace() {
  return useContext(ArtifactWorkspaceContext);
}

function revisionText(instance: UiInstance): string {
  return instance.node.kind === "Markdown"
    ? (instance.node.args[0] ?? "")
    : JSON.stringify(instance.node, null, 2);
}

function ArtifactPanel({ tab }: { tab: ArtifactTab }) {
  const { t } = useTranslation();
  const enabled = useGenUiEnabled();
  const [revisions, setRevisions] = useState<UiViewRevision[]>([]);
  const [revision, setRevision] = useState(tab.instance.revision);
  const [instance, setInstance] = useState(tab.instance);
  const [previous, setPrevious] = useState<UiInstance | null>(null);
  const [showDiff, setShowDiff] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setRevision(tab.instance.revision);
    setInstance(tab.instance);
    setPrevious(null);
    setShowDiff(false);
    void uiApi
      .revisions(tab.instance.viewId)
      .then((value) => active && setRevisions(value))
      .catch(() => active && setFailed(true));
    return () => {
      active = false;
    };
  }, [tab.instance]);

  useEffect(() => {
    let active = true;
    let release: (() => void) | undefined;
    setFailed(false);
    void uiApi
      .load(tab.instance.id, revision)
      .then((value) => {
        if (!active) return;
        release = uiStates.retain(value);
        setInstance(value);
      })
      .catch(() => active && setFailed(true));
    return () => {
      active = false;
      release?.();
    };
  }, [revision, tab.instance.id]);

  useEffect(() => {
    if (!showDiff) {
      setPrevious(null);
      return;
    }
    const index = revisions.findIndex((item) => item.revision === revision);
    const previousRevision = revisions[index + 1]?.revision;
    if (!previousRevision) {
      setPrevious(null);
      return;
    }
    let active = true;
    void uiApi
      .load(tab.instance.id, previousRevision)
      .then((value) => active && setPrevious(value))
      .catch(() => active && setFailed(true));
    return () => {
      active = false;
    };
  }, [revision, revisions, showDiff, tab.instance.id]);

  const diff = useMemo(
    () => (previous ? lineDiff(revisionText(previous), revisionText(instance)) : []),
    [instance, previous],
  );

  if (failed) return <p role="alert">{t("genui.unavailable")}</p>;
  return (
    <UiContext.Provider
      value={{ instance, conversationId: tab.conversationId, active: true, enabled }}
    >
      <div className="artifact-toolbar">
        <label>
          {t("genui.revision")}
          <select value={revision} onChange={(event) => setRevision(Number(event.target.value))}>
            {revisions.map((item) => (
              <option key={item.revision} value={item.revision}>
                v{item.revision} · {item.summary}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className="secondary-button"
          aria-pressed={showDiff}
          disabled={!revisions.some((item) => item.revision < revision)}
          onClick={() => setShowDiff((value) => !value)}
        >
          {t("genui.diff")}
        </button>
      </div>
      {showDiff && previous ? (
        <pre className="artifact-diff" aria-label={t("genui.diff")}>
          {diff.map((line, index) => (
            <span key={`${index}:${line.kind}`} className={`artifact-diff-${line.kind}`}>
              {line.kind === "added" ? "+ " : line.kind === "removed" ? "- " : "  "}
              {line.text || " "}
              {"\n"}
            </span>
          ))}
        </pre>
      ) : (
        <UiBoundary fallback={<p>{t("genui.unavailable")}</p>}>
          <SemanticRenderer node={instance.node} displayMode="artifact" />
        </UiBoundary>
      )}
    </UiContext.Provider>
  );
}

export function ArtifactWorkspaceProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [tabs, setTabs] = useState<ArtifactTab[]>([]);
  const [activeViewId, setActiveViewId] = useState<string | null>(null);
  const open = useCallback((instance: UiInstance, conversationId: string) => {
    setTabs((current) => {
      const existing = current.findIndex((tab) => tab.instance.viewId === instance.viewId);
      if (existing >= 0) {
        return current.map((tab, index) =>
          index === existing ? { instance, conversationId } : tab,
        );
      }
      return [...current, { instance, conversationId }].slice(-8);
    });
    setActiveViewId(instance.viewId);
  }, []);
  const close = (viewId: string) => {
    setTabs((current) => {
      const next = current.filter((tab) => tab.instance.viewId !== viewId);
      if (activeViewId === viewId) {
        setActiveViewId(next.length ? next[next.length - 1].instance.viewId : null);
      }
      return next;
    });
  };
  const active = tabs.find((tab) => tab.instance.viewId === activeViewId) ?? null;
  const width = active ? artifactWidthFor(active.instance.node) : 50;
  return (
    <ArtifactWorkspaceContext.Provider value={{ open }}>
      <div
        className={`artifact-workspace${active ? " artifact-workspace-open" : ""}`}
        style={{ "--artifact-width": `${width}%` } as React.CSSProperties}
      >
        <div className="artifact-chat-region">{children}</div>
        {active && (
          <aside className="artifact-panel" aria-label={t("genui.artifactViewer")}>
            <header className="artifact-panel-header">
              <div className="artifact-tabs" role="tablist">
                {tabs.map((tab) => (
                  <div className="artifact-tab" key={tab.instance.viewId}>
                    <button
                      type="button"
                      role="tab"
                      aria-selected={tab.instance.viewId === activeViewId}
                      onClick={() => setActiveViewId(tab.instance.viewId)}
                    >
                      {tab.instance.name ?? tab.instance.summary}
                    </button>
                    <button
                      type="button"
                      className="artifact-tab-close"
                      aria-label={t("genui.close")}
                      onClick={() => close(tab.instance.viewId)}
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
              <button
                type="button"
                className="secondary-button"
                onClick={() => close(activeViewId!)}
              >
                {t("genui.close")}
              </button>
            </header>
            <div className="artifact-panel-body">
              <h2>{active.instance.name ?? active.instance.summary}</h2>
              <ArtifactPanel key={active.instance.viewId} tab={active} />
            </div>
          </aside>
        )}
      </div>
    </ArtifactWorkspaceContext.Provider>
  );
}
