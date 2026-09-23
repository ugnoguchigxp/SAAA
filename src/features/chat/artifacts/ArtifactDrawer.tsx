import {
  createContext,
  lazy,
  Suspense,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../../components/AppIcon";
import type { UiInstance } from "../../../lib/generated/generativeUi";
import { UiBoundary } from "../ui/UiBoundary";
import {
  artifactTabId,
  artifactTabTitle,
  reduceArtifactWorkspace,
  type InteractivePreviewTab,
  type SourceTab,
} from "./artifactTab";
import { artifactWidthFor } from "./artifactWidth";
import "./artifact.css";

const ArtifactPanel = lazy(() => import("./ArtifactPanel"));
const InteractivePreview = lazy(() => import("./InteractivePreview"));
const SourceArtifact = lazy(() => import("./SourceArtifact"));

type ArtifactWorkspaceContextValue = {
  open: (instance: UiInstance, conversationId: string) => void;
  openInteractivePreview: (tab: Omit<InteractivePreviewTab, "kind">) => void;
  openSource: (tab: Omit<SourceTab, "kind">) => void;
};
const ArtifactWorkspaceContext = createContext<ArtifactWorkspaceContextValue | null>(null);

export function useArtifactWorkspace() {
  return useContext(ArtifactWorkspaceContext);
}

export function ArtifactWorkspaceProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [{ tabs, activeTabId }, dispatch] = useReducer(reduceArtifactWorkspace, {
    tabs: [],
    activeTabId: null,
  });
  const panelRef = useRef<HTMLElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);
  const open = useCallback((instance: UiInstance, conversationId: string) => {
    openerRef.current = document.activeElement as HTMLElement | null;
    dispatch({ type: "open", tab: { kind: "semantic-ui", instance, conversationId } });
  }, []);
  const openInteractivePreview = useCallback((tab: Omit<InteractivePreviewTab, "kind">) => {
    openerRef.current = document.activeElement as HTMLElement | null;
    dispatch({ type: "open", tab: { kind: "interactive-preview", ...tab } });
  }, []);
  const openSource = useCallback((tab: Omit<SourceTab, "kind">) => {
    openerRef.current = document.activeElement as HTMLElement | null;
    dispatch({ type: "open", tab: { kind: "source", ...tab } });
  }, []);
  const contextValue = useMemo(
    () => ({ open, openInteractivePreview, openSource }),
    [open, openInteractivePreview, openSource],
  );
  const close = useCallback((tabId: string) => dispatch({ type: "close", tabId }), []);
  const active = tabs.find((tab) => artifactTabId(tab) === activeTabId) ?? null;
  const width = active?.kind === "semantic-ui" ? artifactWidthFor(active.instance.node) : 50;
  useEffect(() => {
    const isOpen = Boolean(active);
    if (isOpen && !wasOpenRef.current) panelRef.current?.focus();
    else if (!isOpen && wasOpenRef.current) openerRef.current?.focus();
    wasOpenRef.current = isOpen;
  }, [active]);
  useEffect(() => {
    if (!active) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      close(artifactTabId(active));
    };
    window.addEventListener("keydown", closeOnEscape, true);
    return () => window.removeEventListener("keydown", closeOnEscape, true);
  }, [active, close]);
  return (
    <ArtifactWorkspaceContext.Provider value={contextValue}>
      <div
        className={`artifact-workspace${active ? " artifact-workspace-open" : ""}`}
        style={{ "--artifact-width": `${width}%` } as React.CSSProperties}
      >
        <div
          className="artifact-chat-region"
          inert={active && width === 100 ? true : undefined}
          aria-hidden={active && width === 100 ? true : undefined}
        >
          {children}
        </div>
        {active && (
          <aside
            ref={panelRef}
            className="artifact-panel"
            aria-labelledby="artifact-panel-title"
            tabIndex={-1}
          >
            <header className="artifact-panel-header">
              <div className="artifact-tabs" role="tablist">
                {tabs.map((tab, index) => {
                  const title = artifactTabTitle(tab);
                  const tabId = artifactTabId(tab);
                  const selected = tabId === activeTabId;
                  return (
                    <div className="artifact-tab" key={tabId}>
                      <button
                        type="button"
                        role="tab"
                        id={`artifact-tab-${index}`}
                        aria-controls="artifact-panel-content"
                        aria-selected={selected}
                        tabIndex={selected ? 0 : -1}
                        onClick={() => dispatch({ type: "select", tabId })}
                        onKeyDown={(event) => {
                          let nextIndex: number | null = null;
                          if (event.key === "ArrowLeft")
                            nextIndex = (index - 1 + tabs.length) % tabs.length;
                          if (event.key === "ArrowRight") nextIndex = (index + 1) % tabs.length;
                          if (event.key === "Home") nextIndex = 0;
                          if (event.key === "End") nextIndex = tabs.length - 1;
                          if (nextIndex === null) return;
                          event.preventDefault();
                          const next = tabs[nextIndex];
                          dispatch({ type: "select", tabId: artifactTabId(next) });
                          document.getElementById(`artifact-tab-${nextIndex}`)?.focus();
                        }}
                      >
                        {title}
                      </button>
                      <button
                        type="button"
                        className="artifact-tab-close"
                        aria-label={`${t("genui.close")}: ${title}`}
                        title={`${t("genui.close")}: ${title}`}
                        onClick={() => close(tabId)}
                      >
                        <AppIcon name="close" />
                      </button>
                    </div>
                  );
                })}
              </div>
              <button
                type="button"
                className="artifact-panel-close"
                aria-label={t("genui.close")}
                title={t("genui.close")}
                onClick={() => close(activeTabId!)}
              >
                <AppIcon name="close" />
              </button>
            </header>
            <div
              id="artifact-panel-content"
              className="artifact-panel-body"
              role="tabpanel"
              aria-labelledby={`artifact-tab-${tabs.findIndex((tab) => artifactTabId(tab) === activeTabId)}`}
            >
              <h2 id="artifact-panel-title">{artifactTabTitle(active)}</h2>
              {active.kind === "semantic-ui" ? (
                <UiBoundary key={artifactTabId(active)} fallback={<p>{t("genui.unavailable")}</p>}>
                  <Suspense fallback={<p>{t("genui.loading")}</p>}>
                    <ArtifactPanel
                      key={active.instance.viewId}
                      instance={active.instance}
                      conversationId={active.conversationId}
                    />
                  </Suspense>
                </UiBoundary>
              ) : active.kind === "interactive-preview" ? (
                <UiBoundary
                  key={artifactTabId(active)}
                  fallback={<p>{t("genui.previewUnavailable")}</p>}
                >
                  <Suspense fallback={<p>{t("genui.previewLoading")}</p>}>
                    <InteractivePreview
                      artifactId={active.artifactId}
                      revisionId={active.revisionId}
                      title={active.title}
                    />
                  </Suspense>
                </UiBoundary>
              ) : (
                <Suspense fallback={<p>{t("genui.loading")}</p>}>
                  <SourceArtifact conversationId={active.conversationId} url={active.url} />
                </Suspense>
              )}
            </div>
          </aside>
        )}
      </div>
    </ArtifactWorkspaceContext.Provider>
  );
}
