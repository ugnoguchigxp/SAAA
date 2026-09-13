// Deterministic visual fixture; never imported by the application entry point.
import React, { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "../src/i18n";
import "../src/App.css";
import "../src/features/chat/ui/ui.css";
import { uiApi, type UiInstance } from "../src/features/chat/ui/api";
import { VirtualMessages } from "../src/features/chat/VirtualMessages";
import type { ConversationMessage } from "../src/lib/contracts";

const definition = `root = Stack([heading,grid,chart,actions])
heading = Text("この会話の実行状態")
grid = Grid([Cell(metric,4),Cell(models,8)])
metric = Metric("runtime.summary","running","実行中")
models = ModelStatus("larm.status")
chart = Chart("runtime.history","time","count")
actions = Actions("refresh")`;
const leaf = (id: string, kind: string, args: string[]): UiInstance["node"] => ({
  id,
  kind,
  args,
  span: 12,
  children: [],
});
const node: UiInstance["node"] = {
  id: "root",
  kind: "Stack",
  args: [],
  span: 12,
  children: [
    leaf("heading", "Text", ["この会話の実行状態"]),
    {
      id: "grid",
      kind: "Grid",
      args: [],
      span: 12,
      children: [
        {
          id: "left",
          kind: "Cell",
          args: [],
          span: 4,
          children: [leaf("metric", "Metric", ["runtime.summary", "running", "実行中"])],
        },
        {
          id: "right",
          kind: "Cell",
          args: [],
          span: 8,
          children: [leaf("models", "ModelStatus", ["larm.status"])],
        },
      ],
    },
    leaf("chart", "Chart", ["runtime.history", "time", "count"]),
    leaf("actions", "Actions", ["refresh"]),
  ],
};
const states = new Map<string, UiInstance["state"]>();
let queries = 0;
let saved = 0;
uiApi.enabled = async () => true;
uiApi.load = async (id) => ({
  id,
  viewId: "view_test",
  revision: 1,
  summary: "ローカル実行ダッシュボード",
  definition,
  libraryVersion: 1,
  mode: "live",
  node,
  state: states.get(id) ?? {},
  snapshots: {},
  stateVersion: 0,
  name: null,
  publishedRevision: null,
});
uiApi.state = async (id, version, state) => {
  states.set(id, state);
  return version + 1;
};
uiApi.query = async (_id, source) => {
  queries++;
  document.querySelector("#queries")!.textContent = String(queries);
  return {
    capturedAt: String(Date.now()),
    rows:
      source === "runtime.summary"
        ? [{ running: 2, completed: 14, failed: 1, total: 17 }]
        : source === "larm.status"
          ? [
              {
                provider: "Qwen",
                runtime: "local-01",
                status: "running",
                updatedAt: String(Date.now()),
              },
              {
                provider: "Gemma",
                runtime: "local-02",
                status: "running",
                updatedAt: String(Date.now()),
              },
            ]
          : [1, 3, 2, 5, 4, 8].map((count, i) => ({ time: Date.now() - (5 - i) * 60_000, count })),
  };
};
uiApi.save = async () => {
  saved++;
  document.querySelector("#saves")!.textContent = String(saved);
  return { saved: true };
};
uiApi.snapshot = async () => ({ saved: true });
const messages: ConversationMessage[] = Array.from({ length: 150 }, (_, i) => ({
  id: `m${i}`,
  conversationId: "fixture",
  role: "assistant",
  content: `履歴 ${i + 1} · これは表示検証用の固定データです。`,
  createdAt: String(i),
  ...(i % 15 === 0
    ? {
        parts: [
          {
            type: "ui" as const,
            instanceId: `ui_${i}`,
            viewId: "view_test",
            revision: 1,
            summary: "ローカル実行ダッシュボード",
          },
        ],
      }
    : {}),
}));
function Preview() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  const [visibleMessages, setVisibleMessages] = useState(messages);
  function prepend() {
    setVisibleMessages((current) =>
      [
        ...Array.from({ length: 30 }, (_, i) => ({
          id: `before_${current.length}_${i}`,
          conversationId: "fixture",
          role: "assistant",
          content: `過去の記録 ${i} · `.repeat((i % 5) + 1),
          createdAt: String(-current.length + i),
        })),
        ...current,
      ].slice(0, 150),
    );
  }
  return (
    <main style={{ height: "100vh", padding: 20, color: "#edf3fa", background: "#0d1118" }}>
      <header style={{ display: "flex", gap: 16, flexWrap: "wrap" }}>
        <strong>GenUI 検証用データ</strong>
        <label>
          幅{" "}
          <select value={width} onChange={(e) => setWidth(Number(e.target.value))}>
            <option>360</option>
            <option>768</option>
            <option>1000</option>
            <option>1200</option>
          </select>
        </label>
        <button onClick={prepend}>過去30件を追加</button>
        <button
          onClick={() => {
            ref.current!.scrollTop = 0;
          }}
        >
          先頭
        </button>
        <button
          onClick={() => {
            ref.current!.scrollTop = ref.current!.scrollHeight;
          }}
        >
          末尾
        </button>
        <span>
          取得: <b id="queries">0</b> / 保存: <b id="saves">0</b>
        </span>
      </header>
      <div
        className="message-area"
        ref={ref}
        style={{
          height: "85vh",
          width,
          maxWidth: "100%",
          padding: 12,
          margin: "auto",
          position: "relative",
        }}
      >
        <VirtualMessages messages={visibleMessages} scrollRef={ref} />
      </div>
    </main>
  );
}
createRoot(document.getElementById("root")!).render(<Preview />);
