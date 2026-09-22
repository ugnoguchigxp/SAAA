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

export type ArtifactTab = SemanticUiTab | InteractivePreviewTab;

export type ArtifactWorkspaceState = {
  tabs: ArtifactTab[];
  activeTabId: string | null;
};

export type ArtifactWorkspaceAction =
  | { type: "open"; tab: ArtifactTab }
  | { type: "close"; tabId: string }
  | { type: "select"; tabId: string };

export function artifactTabId(tab: ArtifactTab): string {
  return tab.kind === "semantic-ui"
    ? tab.instance.viewId
    : `preview:${tab.artifactId}:${tab.revisionId}`;
}

export function artifactTabTitle(tab: ArtifactTab): string {
  return tab.kind === "semantic-ui" ? (tab.instance.name ?? tab.instance.summary) : tab.title;
}

export function reduceArtifactWorkspace(
  state: ArtifactWorkspaceState,
  action: ArtifactWorkspaceAction,
): ArtifactWorkspaceState {
  if (action.type === "open") {
    const id = artifactTabId(action.tab);
    const existing = state.tabs.findIndex((tab) => artifactTabId(tab) === id);
    const tabs =
      existing >= 0
        ? state.tabs.map((tab, index) => (index === existing ? action.tab : tab))
        : [...state.tabs, action.tab].slice(-8);
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
