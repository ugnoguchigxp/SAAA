import { expect, mock, test } from "bun:test";
import { act } from "react";
import { installJsdom } from "./jsdomGlobals";

let onQueueUpdate: (() => void) | null = null;
let onAsrUpdate: (() => void) | null = null;
let asr = {
  phase: "idle",
  playbackLimited: false,
  speechDetected: false,
  interimText: "",
  error: null,
  entries: [] as Array<{
    id: string;
    status: string;
    text: string;
    provider: string;
    deliveryQueued: boolean;
  }>,
};
let jobs: Array<{ id: string; key: string; kind: string; state: string; error: null }> = [];
let messages: Array<{ id: string; role: string; content: string }> = [];
const enqueued: Array<{ id: string; text: string }> = [];

mock.module("@tauri-apps/api/event", () => ({
  listen: async (name: string, handler: () => void) => {
    if (name === "conversation-queue-updated") onQueueUpdate = handler;
    return () => {
      onQueueUpdate = null;
    };
  },
}));
mock.module("../src/lib/conversationAsrCapture", () => ({
  conversationAsrSnapshot: () => asr,
  subscribeConversationAsr: (listener: () => void) => {
    onAsrUpdate = listener;
    return () => {
      onAsrUpdate = null;
    };
  },
  startConversationAsr: async () => {},
  stopConversationAsr: async () => {},
  setConversationAsrPlaybackActive: () => {},
  queueConversationAsrDelivery: (id: string) => {
    const entry = asr.entries.find((candidate) => candidate.id === id);
    if (!entry || entry.deliveryQueued) return false;
    entry.deliveryQueued = true;
    return true;
  },
  failConversationAsrDelivery: () => {},
  retryConversationAsrDelivery: () => null,
}));
mock.module("../src/lib/runtime", () => ({
  conversationQueueSnapshot: async () => ({ jobs, speechPlaying: false }),
  listMessages: async () => ({ messages }),
  enqueueConversationText: async (id: string, text: string) => {
    enqueued.push({ id, text });
    return { inputId: id, jobId: `job-${id}` };
  },
  cancelConversationInput: async () => {},
  replayConversationSpeech: async () => {},
}));

test("ASR final enters the queue and queue events refresh the route and answer", async () => {
  const environment = installJsdom();
  HTMLElement.prototype.scrollIntoView = () => {};
  const { createRoot } = await import("react-dom/client");
  const { ConversationCheckPage } = await import("../src/features/chat/ConversationCheckPage");
  const root = createRoot(document.getElementById("root")!);
  try {
    await act(async () =>
      root.render(
        <ConversationCheckPage
          conversationId="primary"
          providerLabel="fixture"
          inputDeviceId="default"
          echoCancellation
          listeningEnabled={false}
          vadSensitivity="medium"
          silenceTimeoutMs={1500}
          onOpenSettings={() => {}}
        />,
      ),
    );
    await act(async () => {
      asr = {
        ...asr,
        entries: [
          {
            id: "asr-final-1",
            status: "completed",
            text: "調べて",
            provider: "qwen",
            deliveryQueued: false,
          },
        ],
      };
      onAsrUpdate?.();
      await Promise.resolve();
    });
    expect(enqueued).toEqual([{ id: "asr-final-1", text: "調べて" }]);
    await act(async () => {
      jobs = [
        { id: "job-1", key: "asr-final-1", kind: "ornith_task", state: "running", error: null },
      ];
      onQueueUpdate?.();
      await Promise.resolve();
    });
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Ornith");
    await act(async () => {
      jobs = [{ id: "job-2", key: "asr-final-1", kind: "speech", state: "completed", error: null }];
      messages = [{ id: "reply-1", role: "assistant", content: "結果は42です。" }];
      onQueueUpdate?.();
      await Promise.resolve();
    });
    expect(document.querySelector(".conversation-check-history")?.textContent).toContain(
      "結果は42です。",
    );
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Qwen 2B");
  } finally {
    await act(async () => root.unmount());
    environment.restore();
    onQueueUpdate = null;
    onAsrUpdate = null;
  }
});
