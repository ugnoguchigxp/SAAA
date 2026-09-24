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
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { AppIcon } from "../../../components/AppIcon";
import type { UiInstance } from "../../../lib/generated/generativeUi";
import { UiBoundary } from "../ui/UiBoundary";
import { extractAnswerUrls } from "./answerUrls";
import { registerAnswerPresenter } from "./answerPresentation";
import {
  artifactSessionKey,
  artifactTabId,
  artifactTabTitle,
  emptyArtifactWorkspace,
  nextWebsiteTabIndex,
  planWebviewCommand,
  reduceArtifactSessions,
  type ArtifactSessionStore,
  type InteractivePreviewTab,
  type SourceTab,
} from "./artifactTab";
import { scrollActiveSourceWebview } from "./artifactWebviewHost";
import { artifactWidthFor } from "./artifactWidth";
import "./artifact.css";

const ArtifactPanel = lazy(() => import("./ArtifactPanel"));
const InteractivePreview = lazy(() => import("./InteractivePreview"));
const SourceWebsite = lazy(() => import("./SourceWebsite"));

type ArtifactWorkspaceContextValue = {
  open: (instance: UiInstance, conversationId: string) => void;
  openInteractivePreview: (tab: Omit<InteractivePreviewTab, "kind">) => void;
  openSource: (tab: Omit<SourceTab, "kind">) => void;
  focusConversation: (conversationId: string) => void;
};
const ArtifactWorkspaceContext = createContext<ArtifactWorkspaceContextValue | null>(null);

export function useArtifactWorkspace() {
  return useContext(ArtifactWorkspaceContext);
}

export function ArtifactWorkspaceProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [store, dispatch] = useReducer(reduceArtifactSessions, {
    conversationId: null,
    sessions: {},
  });
  const storeRef = useRef<ArtifactSessionStore>(store);
  storeRef.current = store;
  const conversationRef = useRef<string | null>(null);
  conversationRef.current = store.conversationId;
  const workspace = store.sessions[artifactSessionKey(store.conversationId)] ?? emptyArtifactWorkspace();
  const { tabs, activeTabId } = workspace;
  const presentedRef = useRef<Set<string>>(new Set());
  const panelRef = useRef<HTMLElement>(null);
  const [sourceBrowserError, setSourceBrowserError] = useState(false);
  const [sourceReadyFor, setSourceReadyFor] = useState<string | null>(null);
  const generationRef = useRef(0);
  const pendingRequestRef = useRef<{
    requestId: string;
    conversationId: string;
    expectedTabId?: string;
    removedTabId?: string;
    closeAll?: boolean;
    timeout: number;
  } | null>(null);
  const onSourceReady = useCallback((ready: boolean) => {
    const tabId = activeTabId;
    setSourceReadyFor((current) => ready ? tabId : current === tabId ? null : current);
  }, [activeTabId]);
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
  const focusConversation = useCallback((conversationId: string) => {
    dispatch({ type: "focus", conversationId });
  }, []);
  useEffect(() => {
    registerAnswerPresenter((message) => {
      if (presentedRef.current.has(message.id)) return;
      presentedRef.current.add(message.id);
      const sources = extractAnswerUrls(message.content).map((item) => ({
        kind: "source" as const,
        conversationId: message.conversationId,
        url: item.url,
        title: item.label,
      }));
      if (sources.length === 0) return;
      dispatch({
        type: "present-sources",
        conversationId: message.conversationId,
        sources,
      });
    });
    return () => registerAnswerPresenter(null);
  }, []);
  const contextValue = useMemo(
    () => ({ open, openInteractivePreview, openSource, focusConversation }),
    [open, openInteractivePreview, openSource, focusConversation],
  );
  const close = useCallback((tabId: string) => dispatch({ type: "close", tabId }), []);
  const sourceTabs = tabs.filter((tab) => tab.kind === "source");
  useEffect(() => {
    const pending = pendingRequestRef.current;
    if (!pending) return;
    const changedConversation = pending.conversationId !== store.conversationId;
    const applied = pending.closeAll
      ? sourceTabs.length === 0
      : pending.removedTabId
        ? !tabs.some((tab) => artifactTabId(tab) === pending.removedTabId)
        : activeTabId === pending.expectedTabId;
    if (!changedConversation && !applied) return;
    window.clearTimeout(pending.timeout);
    pendingRequestRef.current = null;
    void invoke("complete_artifact_webview_request", {
      requestId: pending.requestId,
      applied: !changedConversation && applied,
    }).catch(() => undefined);
  }, [store.conversationId, tabs, activeTabId, sourceTabs.length]);
  const sourceSelection = sourceTabs.map(artifactTabId).join("\n") + "\n" + (activeTabId ?? "");
  const reportedSelection = useRef("");
  useEffect(() => {
    if (reportedSelection.current !== sourceSelection) {
      reportedSelection.current = sourceSelection;
      generationRef.current += 1;
    }
    const conversationId = store.conversationId ?? "";
    const selected = sourceTabs.findIndex((tab) => artifactTabId(tab) === activeTabId);
    void invoke("report_artifact_webview_session", {
      conversationId,
      generation: generationRef.current,
      labels: sourceTabs.map(artifactTabTitle),
      selected: selected >= 0 ? selected : null,
      scrollable: sourceReadyFor === activeTabId && selected >= 0,
      mounted: sourceReadyFor === activeTabId && selected >= 0,
    }).catch(() => undefined);
  }, [sourceSelection, sourceReadyFor, store.conversationId, sourceTabs, activeTabId]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (pendingRequestRef.current) return;
      void invoke<{ requestId: string; conversationId: string; generation: number; operation: string; index?: number } | null>(
        "poll_artifact_webview_request",
      )
        .then(async (request) => {
          if (!request) return;
          const currentStore = storeRef.current;
          const conversationId = currentStore.conversationId;
          const current =
            currentStore.sessions[artifactSessionKey(conversationId)] ?? emptyArtifactWorkspace();
          const websiteTabs = current.tabs.filter((tab) => tab.kind === "source");
          const selected = websiteTabs.findIndex((tab) => artifactTabId(tab) === current.activeTabId);
          const needsSelection = request.operation !== "close_all_tabs";
          const stale =
            request.generation !== generationRef.current || (needsSelection && selected < 0);
          if (stale || !conversationId || request.conversationId !== conversationId) {
            await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
            return;
          }
          try {
            if (request.operation === "scroll") {
              await scrollActiveSourceWebview();
              await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: true });
              return;
            } else if (request.operation === "close_all_tabs") {
              if (websiteTabs.length === 0) throw new Error("no-website-tabs");
              pendingRequestRef.current = {
                requestId: request.requestId, conversationId, closeAll: true,
                timeout: window.setTimeout(() => {
                  pendingRequestRef.current = null;
                  void invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
                }, 1400),
              };
              dispatch({ type: "close-source-tabs", conversationId });
            } else if (request.operation === "close_tab") {
              pendingRequestRef.current = {
                requestId: request.requestId, conversationId,
                removedTabId: artifactTabId(websiteTabs[selected]),
                timeout: window.setTimeout(() => {
                  pendingRequestRef.current = null;
                  void invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
                }, 1400),
              };
              dispatch({ type: "close", tabId: artifactTabId(websiteTabs[selected]) });
            } else if (request.operation === "select_tab" && websiteTabs[request.index ?? -1]) {
              const targetId = artifactTabId(websiteTabs[request.index ?? -1]);
              if (targetId === current.activeTabId) {
                await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: true });
                return;
              }
              pendingRequestRef.current = {
                requestId: request.requestId, conversationId,
                expectedTabId: targetId,
                timeout: window.setTimeout(() => {
                  pendingRequestRef.current = null;
                  void invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
                }, 1400),
              };
              dispatch({ type: "select", tabId: targetId });
            } else if (request.operation === "next_tab" || request.operation === "previous_tab") {
              const plan = planWebviewCommand(request.operation, selected, websiteTabs.length);
              if (plan.outcome === "ack-now") {
                await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: true });
                return;
              }
              if (plan.outcome !== "reduce-then-ack") throw new Error("no-website-tabs");
              const delta = request.operation === "next_tab" ? 1 : -1;
              const nextIndex = nextWebsiteTabIndex(selected, websiteTabs.length, delta);
              const next = nextIndex === null ? undefined : websiteTabs[nextIndex];
              if (!next) throw new Error("no-website-tabs");
              if (artifactTabId(next) === current.activeTabId) {
                await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: true });
                return;
              }
              pendingRequestRef.current = {
                requestId: request.requestId, conversationId,
                expectedTabId: artifactTabId(next),
                timeout: window.setTimeout(() => {
                  pendingRequestRef.current = null;
                  void invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
                }, 1400),
              };
              dispatch({ type: "select", tabId: artifactTabId(next) });
            } else {
              throw new Error("webview-operation-unknown");
            }
          } catch {
            const pending = pendingRequestRef.current;
            if (pending?.requestId === request.requestId) {
              window.clearTimeout(pending.timeout);
              pendingRequestRef.current = null;
            }
            await invoke("complete_artifact_webview_request", { requestId: request.requestId, applied: false });
          }
        })
        .catch(() => undefined);
    }, 250);
    return () => window.clearInterval(timer);
  }, []);
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
            <header className={`artifact-panel-header${active.kind === "source" ? " artifact-panel-header-source" : ""}`}>
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
              {active.kind === "source" && (
                <>
                  <span className="artifact-source-url" title={active.url}>
                    {t("genui.sourceUrl")}: {active.url}
                  </span>
                  {sourceBrowserError && <span role="alert">{t("genui.sourceWebsiteFailed")}</span>}
                  <button
                    type="button"
                    className="artifact-source-browser"
                    onClick={() => {
                      setSourceBrowserError(false);
                      void invoke("open_source_website_in_browser", {
                        conversationId: active.conversationId,
                        url: active.url,
                      }).catch(() => setSourceBrowserError(true));
                    }}
                  >
                    {t("genui.sourceOpenBrowser")}
                  </button>
                </>
              )}
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
              className={`artifact-panel-body${active.kind === "source" ? " artifact-panel-body-source" : ""}`}
              role="tabpanel"
              aria-labelledby={`artifact-tab-${tabs.findIndex((tab) => artifactTabId(tab) === activeTabId)}`}
            >
              <h2 id="artifact-panel-title" className={active.kind === "source" ? "visually-hidden" : undefined}>
                {artifactTabTitle(active)}
              </h2>
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
                    <SourceWebsite
                      conversationId={active.conversationId}
                      url={active.url}
                      title={active.title}
                      onReady={onSourceReady}
                    />
                </Suspense>
              )}
            </div>
          </aside>
        )}
      </div>
    </ArtifactWorkspaceContext.Provider>
  );
}
