import type { UiInstance } from "../../../lib/generated/generativeUi";

export type SemanticUiTab = {
  kind: "semantic-ui";
  conversationId: string;
  instance: UiInstance;
};

export type InteractivePreviewTab = {
  kind: "interactive-preview";
  artifactId: string;
  revisionId: string;
  title: string;
};

export type SourceTab = {
  kind: "source";
  conversationId: string;
  url: string;
  title: string;
};

export type ArtifactTab = SemanticUiTab | InteractivePreviewTab | SourceTab;

export type ArtifactWorkspaceState = {
  tabs: ArtifactTab[];
  activeTabId: string | null;
};

export type ArtifactWorkspaceAction =
  | { type: "open"; tab: ArtifactTab }
  | { type: "close"; tabId: string }
  | { type: "select"; tabId: string }
  | { type: "present-sources"; conversationId: string; sources: SourceTab[] }
  | { type: "close-source-tabs"; conversationId: string }
  | { type: "replace"; tabs: ArtifactTab[]; activeTabId: string | null };

export type ArtifactSessionStore = {
  conversationId: string | null;
  sessions: Record<string, ArtifactWorkspaceState>;
};

export const emptyArtifactWorkspace = (): ArtifactWorkspaceState => ({
  tabs: [],
  activeTabId: null,
});

export function artifactSessionKey(conversationId: string | null): string {
  return conversationId ?? "";
}

export function artifactTabId(tab: ArtifactTab): string {
  if (tab.kind === "semantic-ui") return tab.instance.viewId;
  if (tab.kind === "source") return `source:${tab.conversationId}:${tab.url}`;
  return `preview:${tab.artifactId}:${tab.revisionId}`;
}

export function artifactTabTitle(tab: ArtifactTab): string {
  return tab.kind === "semantic-ui" ? (tab.instance.name ?? tab.instance.summary) : tab.title;
}

export function reduceArtifactWorkspace(
  state: ArtifactWorkspaceState,
  action: ArtifactWorkspaceAction,
): ArtifactWorkspaceState {
  if (action.type === "replace") return { tabs: action.tabs, activeTabId: action.activeTabId };
  if (action.type === "present-sources") {
    if (action.sources.length === 0) return state;
    const incoming = action.sources.filter((tab) => tab.conversationId === action.conversationId);
    if (incoming.length === 0) return state;
    const tabs = [...state.tabs];
    for (const source of incoming) {
      const id = artifactTabId(source);
      const index = tabs.findIndex((tab) => artifactTabId(tab) === id);
      if (index >= 0) tabs[index] = source;
      else tabs.push(source);
    }
    return { tabs, activeTabId: artifactTabId(incoming[0]) };
  }
  if (action.type === "close-source-tabs") {
    const tabs = state.tabs.filter(
      (tab) => tab.kind !== "source" || tab.conversationId !== action.conversationId,
    );
    const activeStillOpen = tabs.some((tab) => artifactTabId(tab) === state.activeTabId);
    return {
      tabs,
      activeTabId: activeStillOpen
        ? state.activeTabId
        : tabs.length
          ? artifactTabId(tabs[tabs.length - 1])
          : null,
    };
  }
  if (action.type === "open") {
    const id = artifactTabId(action.tab);
    const existing = state.tabs.findIndex((tab) => artifactTabId(tab) === id);
    const tabs =
      existing >= 0
        ? state.tabs.map((tab, index) => (index === existing ? action.tab : tab))
        : capNonSourceTabs([...state.tabs, action.tab]);
    return { tabs, activeTabId: id };
  }
  if (action.type === "select") {
    return state.tabs.some((tab) => artifactTabId(tab) === action.tabId)
      ? { ...state, activeTabId: action.tabId }
      : state;
  }
  const tabs = state.tabs.filter((tab) => artifactTabId(tab) !== action.tabId);
  return {
    tabs,
    activeTabId:
      state.activeTabId === action.tabId
        ? tabs.length
          ? artifactTabId(tabs[tabs.length - 1])
          : null
        : state.activeTabId,
  };
}

export function reduceArtifactSessions(
  state: ArtifactSessionStore,
  action: ArtifactWorkspaceAction | { type: "focus"; conversationId: string },
): ArtifactSessionStore {
  if (action.type === "focus") {
    return state.conversationId === action.conversationId
      ? state
      : { ...state, conversationId: action.conversationId };
  }
  const key =
    action.type === "present-sources" || action.type === "close-source-tabs"
      ? action.conversationId
      : artifactSessionKey(state.conversationId);
  const current = state.sessions[key] ?? emptyArtifactWorkspace();
  return {
    ...state,
    sessions: {
      ...state.sessions,
      [key]: reduceArtifactWorkspace(current, action),
    },
  };
}

function capNonSourceTabs(tabs: ArtifactTab[]): ArtifactTab[] {
  const nonSource = tabs.filter((tab) => tab.kind !== "source");
  const dropped = new Set(
    nonSource.slice(0, Math.max(0, nonSource.length - 8)).map((tab) => artifactTabId(tab)),
  );
  return tabs.filter((tab) => tab.kind === "source" || !dropped.has(artifactTabId(tab)));
}
