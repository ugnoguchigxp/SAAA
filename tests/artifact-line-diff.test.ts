import { describe, expect, test } from "bun:test";
import { artifactWidthFor } from "../src/features/chat/artifacts/artifactWidth";
import { lineDiff } from "../src/features/chat/artifacts/lineDiff";
import type { UiNode } from "../src/lib/generated/generativeUi";

describe("artifact presentation policies", () => {
  test("uses the current 50 percent boundary for every artifact kind", () => {
    for (const kind of ["Markdown", "Grid", "Table"]) {
      const node: UiNode = { id: kind, kind, args: [], span: 12, children: [] };
      expect(artifactWidthFor(node)).toBe(50);
    }
  });

  test("marks added and removed lines across revisions", () => {
    expect(lineDiff("title\nold\nshared", "title\nnew\nshared")).toEqual([
      { kind: "same", text: "title" },
      { kind: "added", text: "new" },
      { kind: "removed", text: "old" },
      { kind: "same", text: "shared" },
    ]);
  });
});
