import { afterEach, expect, test } from "bun:test";
import { act } from "react";
import { WorldReviewMaintenance } from "../src/features/memory/WorldReviewMaintenance";
import { personalStateSnapshotSchema } from "../src/features/memory/api";
import { installJsdom } from "./jsdomGlobals";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
afterEach(resetTauriCoreMock);
test("retrospective controls persist mode and inspect evidence without turning preview into active knowledge", async () => {
  const env = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(document.getElementById("root")!);
  const snapshot = personalStateSnapshotSchema.parse({
    enabled: true,
    revision: 1,
    inputEpoch: 1,
    pendingCount: 0,
    pendingBytes: 0,
    contractReady: true,
    contractReason: null,
    cleanup: [],
    items: [],
    worldItems: [],
    maintenance: {
      reason: "ready",
      failures: 0,
      nextAttemptAt: 0,
      updatedAt: 0,
      retrospective: {
        mode: "preview",
        stages: [
          { stage: "preview", reason: "selected", count: 1, updatedAt: 1 },
        ],
      },
    },
  });
  const modes: string[] = [];
  invokeImpl.handler = async (command, args) => {
    if (command === "set_world_review_mode") {
      modes.push(String(args?.mode));
      return {
        ...snapshot,
        maintenance: {
          ...snapshot.maintenance,
          retrospective: { mode: args?.mode, stages: [] },
        },
      };
    }
    if (command === "world_review_candidates")
      return [
        {
          id: 1,
          scope: "project:sample",
          reason: "selected",
          candidates: [
            {
              kind: "world_relation",
              payload: {
                conditions: [{ key: "first-run", value: "初回のみ" }],
              },
              quote: "初回のみ速くなった",
            },
          ],
        },
      ];
    throw new Error(command);
  };
  try {
    await act(async () =>
      root.render(
        <WorldReviewMaintenance
          snapshot={snapshot}
          setError={(error) => {
            if (error) throw new Error(error);
          }}
        />,
      ),
    );
    expect(document.querySelector<HTMLSelectElement>("select")?.value).toBe(
      "preview",
    );
    await act(async () =>
      document.querySelector<HTMLButtonElement>("button")!.click(),
    );
    expect(document.querySelector("blockquote")?.textContent).toBe(
      "初回のみ速くなった",
    );
    expect(snapshot.worldItems).toHaveLength(0);
    await act(async () => {
      const select = document.querySelector<HTMLSelectElement>("select")!;
      select.value = "apply";
      select.dispatchEvent(new window.Event("change", { bubbles: true }));
    });
    expect(modes).toEqual(["apply"]);
    expect(document.querySelector<HTMLSelectElement>("select")?.value).toBe(
      "apply",
    );
  } finally {
    await act(async () => root.unmount());
    env.restore();
  }
});
