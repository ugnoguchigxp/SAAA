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
import { artifactWidthFor } from "./artifactWidth";
import "./artifact.css";

const ArtifactPanel = lazy(() => import("./ArtifactPanel"));

type ArtifactTab = {
  conversationId: string;
  instance: UiInstance;
};
type ArtifactWorkspaceState = {
  tabs: ArtifactTab[];
  activeViewId: string | null;
};
type ArtifactWorkspaceAction =
  | { type: "open"; tab: ArtifactTab }
  | { type: "close"; viewId: string }
  | { type: "select"; viewId: string };
type ArtifactWorkspaceContextValue = {
  open: (instance: UiInstance, conversationId: string) => void;
};
const ArtifactWorkspaceContext = createContext<ArtifactWorkspaceContextValue | null>(null);

function reduceWorkspace(
  state: ArtifactWorkspaceState,
  action: ArtifactWorkspaceAction,
): ArtifactWorkspaceState {
  if (action.type === "open") {
    const existing = state.tabs.findIndex(
      (tab) => tab.instance.viewId === action.tab.instance.viewId,
    );
    const tabs =
      existing >= 0
        ? state.tabs.map((tab, index) => (index === existing ? action.tab : tab))
        : [...state.tabs, action.tab].slice(-8);
    return { tabs, activeViewId: action.tab.instance.viewId };
  }
  if (action.type === "select") {
    return state.tabs.some((tab) => tab.instance.viewId === action.viewId)
      ? { ...state, activeViewId: action.viewId }
      : state;
  }
  const tabs = state.tabs.filter((tab) => tab.instance.viewId !== action.viewId);
  return {
    tabs,
    activeViewId:
      state.activeViewId === action.viewId
        ? tabs.length
          ? tabs[tabs.length - 1].instance.viewId
          : null
        : state.activeViewId,
  };
}

export function useArtifactWorkspace() {
  return useContext(ArtifactWorkspaceContext);
}

export function ArtifactWorkspaceProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [{ tabs, activeViewId }, dispatch] = useReducer(reduceWorkspace, {
    tabs: [],
    activeViewId: null,
  });
  const panelRef = useRef<HTMLElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const wasOpenRef = useRef(false);
  const open = useCallback((instance: UiInstance, conversationId: string) => {
    openerRef.current = document.activeElement as HTMLElement | null;
    dispatch({ type: "open", tab: { instance, conversationId } });
  }, []);
  const contextValue = useMemo(() => ({ open }), [open]);
  const close = useCallback((viewId: string) => dispatch({ type: "close", viewId }), []);
  const active = tabs.find((tab) => tab.instance.viewId === activeViewId) ?? null;
  const width = active ? artifactWidthFor(active.instance.node) : 50;
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
      close(active.instance.viewId);
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
                  const title = tab.instance.name ?? tab.instance.summary;
                  const selected = tab.instance.viewId === activeViewId;
                  return (
                    <div className="artifact-tab" key={tab.instance.viewId}>
                      <button
                        type="button"
                        role="tab"
                        id={`artifact-tab-${index}`}
                        aria-controls="artifact-panel-content"
                        aria-selected={selected}
                        tabIndex={selected ? 0 : -1}
                        onClick={() =>
                          dispatch({
                            type: "select",
                            viewId: tab.instance.viewId,
                          })
                        }
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
                          dispatch({
                            type: "select",
                            viewId: next.instance.viewId,
                          });
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
                        onClick={() => close(tab.instance.viewId)}
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
                onClick={() => close(activeViewId!)}
              >
                <AppIcon name="close" />
              </button>
            </header>
            <div
              id="artifact-panel-content"
              className="artifact-panel-body"
              role="tabpanel"
              aria-labelledby={`artifact-tab-${tabs.findIndex((tab) => tab.instance.viewId === activeViewId)}`}
            >
              <h2 id="artifact-panel-title">{active.instance.name ?? active.instance.summary}</h2>
              <UiBoundary key={active.instance.viewId} fallback={<p>{t("genui.unavailable")}</p>}>
                <Suspense fallback={<p>{t("genui.loading")}</p>}>
                  <ArtifactPanel
                    key={active.instance.viewId}
                    instance={active.instance}
                    conversationId={active.conversationId}
                  />
                </Suspense>
              </UiBoundary>
            </div>
          </aside>
        )}
      </div>
    </ArtifactWorkspaceContext.Provider>
  );
}
