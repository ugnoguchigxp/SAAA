import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import type { MediaHistory, MediaOutput } from "../src/features/media/mediaApi";

test("stored predictions are reconciled and reopened without creating a new generation", async () => {
  const environment = installJsdom();
  const { createRoot } = await import("react-dom/client");
  const { MediaGenerationPanel } = await import("../src/features/media/MediaGenerationPanel");
  const output: MediaOutput = {
    runId: "stored-run",
    error: null,
    result: {
      kind: "image",
      model: "owner/image",
      jobId: "stored-job",
      artifacts: [
        {
          id: "stored-artifact",
          contentUrl: "https://replicate.delivery/file.png",
          metadataUrl: null,
          mimeType: "image/png",
          metadata: {},
        },
      ],
    },
  };
  let history: MediaHistory = [
    {
      runId: "stored-run",
      kind: "image",
      connectionLabel: "Saved service",
      model: "owner/image",
      status: "unknown",
      jobId: "stored-job",
      result: null,
      error: {
        kind: "outcomeUnknown",
        code: "interrupted",
        retryable: false,
        mayHaveGenerated: true,
        jobId: "stored-job",
      },
      updatedAt: "2026-10-05T00:00:00Z",
    },
  ];
  let submitted = 0;
  let reconciled = 0;
  let read = 0;
  const api = {
    generateMedia: async () => {
      submitted++;
      return output;
    },
    cancelMedia: async () => {},
    readMediaArtifact: async () => {
      read++;
      return new Uint8Array([1, 2, 3]).buffer;
    },
    listMediaGenerations: async () => history,
    reconcileMedia: async (run: string) => {
      expect(run).toBe("stored-run");
      reconciled++;
      history = [{ ...history[0], status: "accepted", result: output.result, error: null }];
      return output;
    },
  };
  const oldCreate = URL.createObjectURL;
  const oldRevoke = URL.revokeObjectURL;
  URL.createObjectURL = () => "blob:stored";
  URL.revokeObjectURL = () => {};
  let root = createRoot(document.getElementById("root")!);
  try {
    await act(async () => root.render(<MediaGenerationPanel api={api} />));
    expect(document.body.textContent).toContain("結果未確認");
    await act(async () =>
      [...document.querySelectorAll("button")]
        .find((b) => b.textContent === "進行状況を照会")!
        .click(),
    );
    expect(submitted).toBe(0);
    expect(reconciled).toBe(1);
    expect(read).toBe(1);
    expect(document.querySelector("img")?.getAttribute("src")).toBe("blob:stored");
    await act(async () => root.unmount());
    root = createRoot(document.getElementById("root")!);
    await act(async () => root.render(<MediaGenerationPanel api={api} />));
    await act(async () =>
      [...document.querySelectorAll("button")]
        .find((b) => b.textContent === "成果物を開く")!
        .click(),
    );
    expect(submitted).toBe(0);
    expect(reconciled).toBe(1);
    expect(read).toBe(2);
  } finally {
    await act(async () => root.unmount());
    URL.createObjectURL = oldCreate;
    URL.revokeObjectURL = oldRevoke;
    environment.restore();
  }
});
