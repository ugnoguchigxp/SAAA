import { expect, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";
import type { MediaOutput, MediaProgress } from "../src/features/media/mediaApi";

const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
let complete: ((value: MediaOutput) => void) | null = null;
let progress: ((value: MediaProgress) => void) | null = null;
let result: Omit<MediaOutput, "runId"> | null = null;
let artifactFails = false;
const api = {
  generateMedia: async (
    input: { runId: string; kind: "image" | "music"; prompt: string },
    onProgress: (event: MediaProgress) => void,
  ): Promise<MediaOutput> => {
    calls.push({ command: "generate_media", args: { input } });
    progress = onProgress;
    if (result) return { ...result, runId: input.runId };
    return new Promise((resolve) => {
      complete = resolve;
    });
  },
  cancelMedia: async (runId: string): Promise<void> => {
    calls.push({ command: "cancel_media_generation", args: { runId } });
  },
  readMediaArtifact: async (runId: string, artifactIndex: number): Promise<ArrayBuffer> => {
    calls.push({ command: "read_generated_media", args: { runId, artifactIndex } });
    if (artifactFails) throw new Error("成果物の取得に失敗");
    return new Uint8Array([1, 2, 3]).buffer;
  },
};

const success: Omit<MediaOutput, "runId"> = {
  error: null,
  result: {
    kind: "image",
    model: "discovered-model",
    jobId: null,
    artifacts: [
      {
        id: "artifact-1",
        contentUrl: "/content",
        metadataUrl: null,
        mimeType: "image/png",
        metadata: {},
      },
    ],
  },
};
async function mount() {
  calls.length = 0;
  result = null;
  complete = null;
  progress = null;
  artifactFails = false;
  const environment = installJsdom();
  const previousCreate = URL.createObjectURL;
  const previousRevoke = URL.revokeObjectURL;
  URL.createObjectURL = () => "blob:media-test";
  URL.revokeObjectURL = () => {};
  const { createRoot } = await import("react-dom/client");
  const { MediaGenerationPanel } = await import("../src/features/media/MediaGenerationPanel");
  const root = createRoot(document.getElementById("root")!);
  await act(async () => root.render(<MediaGenerationPanel api={api} />));
  const textarea = document.querySelector("textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")!.set!.call(
      textarea,
      "生成内容",
    );
    textarea.dispatchEvent(new window.Event("input", { bubbles: true }));
  });
  return {
    close: async () => {
      await act(async () => root.unmount());
      URL.createObjectURL = previousCreate;
      URL.revokeObjectURL = previousRevoke;
      environment.restore();
    },
  };
}
async function submit() {
  await act(async () =>
    document
      .querySelector("form")!
      .dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })),
  );
}
function finish(value: Omit<MediaOutput, "runId">) {
  const input = calls.find((call) => call.command === "generate_media")!.args.input as {
    runId: string;
  };
  complete!({ ...value, runId: input.runId });
}

test("cold wait and cancellation are visible without an automatic generation retry", async () => {
  const mounted = await mount();
  try {
    expect(calls).toHaveLength(0);
    await submit();
    await act(async () => progress!({ phase: "starting", jobId: null, progress: null }));
    expect(document.body.textContent).toContain("起動には数分");
    await act(async () =>
      Array.from(document.querySelectorAll("button"))
        .find((button) => button.textContent === "中止")!
        .click(),
    );
    expect(calls.filter((call) => call.command === "cancel_media_generation")).toHaveLength(1);
    await act(async () =>
      finish({
        result: null,
        error: {
          kind: "cancelled",
          code: "image_wait_cancelled",
          retryable: false,
          mayHaveGenerated: true,
          jobId: null,
        },
      }),
    );
    expect(document.body.textContent).toContain("重複生成");
    expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(1);
  } finally {
    await mounted.close();
  }
});

test("conflict explains failure and retry occurs only when the user requests it", async () => {
  const mounted = await mount();
  try {
    await submit();
    await act(async () =>
      finish({
        result: null,
        error: {
          kind: "conflict",
          code: "busy",
          retryable: true,
          mayHaveGenerated: false,
          jobId: null,
        },
      }),
    );
    expect(document.body.textContent).toContain("競合し、生成に失敗");
    expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(1);
    result = success;
    await act(async () =>
      Array.from(document.querySelectorAll("button"))
        .find((button) => button.textContent === "生成を再試行")!
        .click(),
    );
    expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(2);
    expect(document.querySelector("img")?.getAttribute("src")).toBe("blob:media-test");
  } finally {
    await mounted.close();
  }
});

test("artifact retrieval failure keeps generation success and retries retrieval only", async () => {
  const mounted = await mount();
  try {
    artifactFails = true;
    await submit();
    await act(async () => finish(success));
    expect(document.body.textContent).toContain("生成成功");
    expect(document.body.textContent).toContain("成果物の取得に失敗");
    artifactFails = false;
    await act(async () =>
      Array.from(document.querySelectorAll("button"))
        .find((button) => button.textContent === "成果物の取得を再試行")!
        .click(),
    );
    expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(1);
    expect(calls.filter((call) => call.command === "read_generated_media")).toHaveLength(2);
  } finally {
    await mounted.close();
  }
});

test("startup, generation, timeout and unknown outcomes show distinct failure states", async () => {
  const cases = [
    ["startupFailed", "起動できません", true, false],
    ["generationFailed", "生成に失敗", true, false],
    ["timeout", "待機時間を超え", false, true],
    ["outcomeUnknown", "状態を確認できません", false, true],
  ] as const;
  for (const [kind, message, retryable, mayHaveGenerated] of cases) {
    const mounted = await mount();
    try {
      await submit();
      await act(async () =>
        finish({
          result: null,
          error: { kind, code: "fixture", retryable, mayHaveGenerated, jobId: "music-job" },
        }),
      );
      expect(document.body.textContent).toContain(message);
      expect(document.body.textContent?.includes("生成を再試行")).toBe(retryable);
      expect(document.body.textContent?.includes("重複生成")).toBe(mayHaveGenerated);
      expect(document.body.textContent).toContain("music-job");
      expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(1);
    } finally {
      await mounted.close();
    }
  }
});

test("music loading and encoding lead to playable audio and an mp3 download", async () => {
  const mounted = await mount();
  try {
    const select = document.querySelector("select")!;
    await act(async () => {
      select.value = "music";
      select.dispatchEvent(new window.Event("change", { bubbles: true }));
    });
    await submit();
    for (const [phase, label] of [
      ["loading", "楽曲モデルを起動中"],
      ["encoding", "音声ファイルを作成中"],
    ]) {
      await act(async () => progress!({ phase, jobId: "music-job", progress: 0.5 }));
      expect(document.body.textContent).toContain(label);
      expect(document.body.textContent).toContain("50%");
    }
    await act(async () =>
      finish({
        error: null,
        result: {
          kind: "music",
          model: "discovered-music",
          jobId: "music-job",
          artifacts: [
            {
              id: "music-artifact",
              contentUrl: "/audio",
              metadataUrl: "/metadata",
              mimeType: "audio/mpeg",
              metadata: {},
            },
          ],
        },
      }),
    );
    expect(document.querySelector("audio")?.getAttribute("src")).toBe("blob:media-test");
    expect(document.querySelector("a")?.download).toBe("music-artifact.mp3");
    expect(calls.filter((call) => call.command === "generate_media")).toHaveLength(1);
  } finally {
    await mounted.close();
  }
});

test("leaving a pending generation requests cancellation and ignores its late result", async () => {
  const mounted = await mount();
  await submit();
  await mounted.close();
  expect(calls.filter((call) => call.command === "cancel_media_generation")).toHaveLength(1);
  finish(success);
  await Promise.resolve();
  expect(calls.filter((call) => call.command === "read_generated_media")).toHaveLength(0);
});
