import { describe, expect, test } from "bun:test";
import type { UiInstance } from "../src/lib/generated/generativeUi";
import {
  artifactTabId,
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
});
