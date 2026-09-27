import { describe, expect, test } from "bun:test";
import type { UiInstance } from "../src/lib/generated/generativeUi";
import {
  artifactTabId,
  nextWebsiteTabIndex,
  planWebviewCommand,
  reduceArtifactSessions,
  reduceArtifactWorkspace,
  type ArtifactTab,
} from "../src/features/chat/artifacts/artifactTab";

const instance = (viewId: string, summary: string): UiInstance => ({
  id: viewId,
  viewId,
  revision: 1,
  summary,
  definition: "",
  libraryVersion: 1,
  mode: "snapshot",
  node: { id: "root", kind: "Text", args: ["x"], span: 12, children: [] },
  state: {},
  snapshots: {},
  stateVersion: 0,
  name: null,
  publishedRevision: null,
});

function semantic(viewId: string): ArtifactTab {
  return { kind: "semantic-ui", conversationId: "c1", instance: instance(viewId, viewId) };
}

function preview(id: string): ArtifactTab {
  return {
    kind: "interactive-preview",
    artifactId: id,
    revisionId: "rev.1",
    title: id,
  };
}

describe("artifact tab reducer", () => {
  test("mixes tab kinds, caps at eight, and falls back on close", () => {
    let state = { tabs: [] as ArtifactTab[], activeTabId: null as string | null };
    state = reduceArtifactWorkspace(state, { type: "open", tab: semantic("s1") });
    state = reduceArtifactWorkspace(state, { type: "open", tab: preview("p1") });
    expect(state.tabs.map(artifactTabId)).toEqual(["s1", "preview:p1:rev.1"]);
    state = reduceArtifactWorkspace(state, { type: "select", tabId: "s1" });
    expect(state.activeTabId).toBe("s1");
    for (let index = 2; index <= 8; index += 1) {
      state = reduceArtifactWorkspace(state, { type: "open", tab: semantic(`s${index}`) });
    }
    expect(state.tabs).toHaveLength(8);
    state = reduceArtifactWorkspace(state, { type: "open", tab: semantic("s9") });
    expect(state.tabs).toHaveLength(8);
    expect(state.tabs.map(artifactTabId)).not.toContain("s1");
    const last = artifactTabId(state.tabs[state.tabs.length - 1]);
    state = reduceArtifactWorkspace(state, { type: "close", tabId: last });
    expect(state.activeTabId).toBe(artifactTabId(state.tabs[state.tabs.length - 1]));
  });

  test("keeps every website tab from an answer", () => {
    let state = { tabs: [] as ArtifactTab[], activeTabId: null as string | null };
    const sources = Array.from({ length: 9 }, (_, index) => ({
      kind: "source" as const,
      conversationId: "c1",
      url: `https://example.com/${index}`,
      title: `site-${index}`,
    }));
    state = reduceArtifactWorkspace(state, {
      type: "present-sources",
      conversationId: "c1",
      sources,
    });
    expect(state.tabs).toHaveLength(9);
    expect(state.activeTabId).toBe("source:c1:https://example.com/0");
    state = reduceArtifactWorkspace(state, {
      type: "present-sources",
      conversationId: "c1",
      sources: [],
    });
    expect(state.activeTabId).toBe("source:c1:https://example.com/0");
    state = reduceArtifactWorkspace(state, { type: "close-source-tabs", conversationId: "c1" });
    expect(state.tabs).toHaveLength(0);
  });

  test("a single website tab stays selected when moving next", () => {
    expect(nextWebsiteTabIndex(0, 1, 1)).toBe(0);
    expect(nextWebsiteTabIndex(0, 2, 1)).toBe(1);
    expect(nextWebsiteTabIndex(0, 0, 1)).toBeNull();
    expect(planWebviewCommand("next_tab", 0, 1)).toEqual({ outcome: "ack-now" });
    expect(planWebviewCommand("next_tab", 0, 2)).toEqual({
      outcome: "reduce-then-ack",
      action: "select",
    });
    expect(planWebviewCommand("scroll", 0, 1)).toEqual({ outcome: "scroll-then-ack" });
    expect(planWebviewCommand("close_all_tabs", 0, 2)).toEqual({
      outcome: "reduce-then-ack",
      action: "close-all",
    });
  });

  test("keeps website tabs on the conversation that produced them", () => {
    let store = reduceArtifactSessions(
      { conversationId: "c1", sessions: {} },
      {
        type: "present-sources",
        conversationId: "c2",
        sources: [
          { kind: "source", conversationId: "c2", url: "https://example.com/a", title: "A" },
        ],
      },
    );
    expect(store.sessions.c1).toBeUndefined();
    expect(store.sessions.c2?.tabs).toHaveLength(1);
    store = reduceArtifactSessions(store, { type: "focus", conversationId: "c2" });
    expect(store.conversationId).toBe("c2");
    expect(store.sessions.c2?.activeTabId).toBe("source:c2:https://example.com/a");
  });
});
