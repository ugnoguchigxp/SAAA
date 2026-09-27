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
  error: null as string | null,
  entries: [] as Array<{
    id: string;
    status: string;
    text: string;
    provider: string;
    deliveryQueued: boolean;
  }>,
};
let jobs: Array<{ id: string; key: string; kind: string; state: string; error: string | null }> = [];
let messages: Array<{ id: string; role: string; content: string }> = [];
const enqueued: Array<{ id: string; text: string }> = [];
const openedSources: Array<{ conversationId: string; url: string; title: string }> = [];
let idleStarts = 0;
let idleStops = 0;

mock.module("../src/features/chat/artifacts/ArtifactDrawer", () => ({
  useArtifactWorkspace: () => ({
    openSource: (source: { conversationId: string; url: string; title: string }) => openedSources.push(source),
  }),
}));

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
  startConversationAudioIdle: async () => { idleStarts += 1; },
  stopConversationAudioIdle: async () => { idleStops += 1; },
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
          agentName="セバスチャン"
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
    expect(idleStarts).toBe(1);
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
    expect(document.querySelector('.conversation-thinking[aria-label="思考中"]')).not.toBeNull();
    await act(async () => {
      asr = { ...asr, phase: "recording", playbackLimited: true, speechDetected: true };
      onAsrUpdate?.();
    });
    expect(document.querySelector('.voice-activity-indicator.detecting[aria-label="音声を認識中"]')).not.toBeNull();
    await act(async () => {
      jobs = [{ id: "job-failed", key: "asr-final-1", kind: "ornith_task", state: "failed", error: "credential rejected" }];
      onQueueUpdate?.();
      await Promise.resolve();
    });
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("会話処理: credential rejected");
    expect(document.querySelector('[role="status"]')?.textContent).not.toContain("回答を作成中");
    expect(document.querySelector('.conversation-thinking')).toBeNull();
    await act(async () => {
      asr = { ...asr, error: "larm_provider_not_claimable" };
      onAsrUpdate?.();
    });
    expect(document.querySelectorAll('[role="alert"]')).toHaveLength(2);
    expect(document.body.textContent).toContain("音声認識: larm_provider_not_claimable");
    await act(async () => {
      asr = { ...asr, error: null };
      onAsrUpdate?.();
    });
    await act(async () => {
      jobs = [{ id: "job-2", key: "asr-final-1", kind: "speech", state: "completed", error: null }];
      messages = [{ id: "reply-1", role: "assistant", content: "結果は42です。\n\n<!-- saaa:source-links -->\n[出典1: example.com](https://example.com/report)\n" }];
      onQueueUpdate?.();
      await Promise.resolve();
    });
    expect(document.querySelector('[role="alert"]')).toBeNull();
    expect(document.querySelector(".conversation-check-history")?.textContent).toContain(
      "結果は42です。",
    );
    expect(document.querySelector(".conversation-check-message.assistant strong")?.textContent).toBe("セバスチャン");
    expect(document.querySelector(".conversation-check-history")?.textContent).not.toContain("saaa:source-links");
    expect(document.querySelector<HTMLAnchorElement>(".conversation-check-sources a")?.href).toBe("https://example.com/report");
    document.querySelector<HTMLAnchorElement>(".conversation-check-sources a")?.click();
    expect(openedSources).toEqual([{ conversationId: "primary", url: "https://example.com/report", title: "出典1: example.com" }]);
    expect(document.querySelector('[role="status"]')?.textContent).toContain("Qwen 2B");
  } finally {
    await act(async () => root.unmount());
    await Promise.resolve();
    expect(idleStops).toBe(1);
    environment.restore();
    onQueueUpdate = null;
    onAsrUpdate = null;
  }
});
