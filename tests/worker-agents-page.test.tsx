import { afterEach, beforeEach, expect, test } from "bun:test";
import { act } from "react";
import type { Root } from "react-dom/client";
import i18n from "../src/i18n";
import type {
  BlocklistEntry,
  RevisionSummary,
  WorkerAgentSummary,
  WorkerTaskSummary,
} from "../src/lib/generated/workerAgents";
import { installJsdom } from "./jsdomGlobals";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

const { WorkerAgentsPage } = await import("../src/features/workerAgents/WorkerAgentsPage");

const revision = (over: Partial<RevisionSummary> = {}): RevisionSummary => ({
  revisionId: "rev-1",
  revision: 1,
  reviewState: "approved",
  purpose: "ウェブを調べる",
  definitionHash: "hash-1",
  toolKeys: ["web_search", "fetch_content"],
  outputKind: "web_claims_v1",
  createdAtMs: 1_700_000_000_000,
  ...over,
});

let agents: WorkerAgentSummary[];
let tasks: WorkerTaskSummary[];
let blocklist: BlocklistEntry[];
let fail: string | null;

beforeEach(() => {
  resetTauriCoreMock();
  fail = null;
  agents = [
    {
      profileId: "web_search",
      origin: "builtin",
      enabled: true,
      pinnedOffer: false,
      current: revision(),
      draft: revision({
        revisionId: "rev-2",
        revision: 2,
        reviewState: "draft",
        purpose: "新しい目的",
        definitionHash: "hash-2",
        toolKeys: ["web_search"],
      }),
    },
  ];
  tasks = [
    {
      taskId: "task-run",
      profileId: "web_search",
      state: "running",
      delivery: "sync_waiting",
      failureCode: null,
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_001_000,
    },
    {
      taskId: "task-fail",
      profileId: "web_search",
      state: "failed",
      delivery: "async_delivered",
      failureCode: "no_safe_sources",
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_001_000,
    },
  ];
  blocklist = [
    { urlHash: "uh-1", host: "evil.example", reason: "injection", createdAtMs: 1_700_000_000_000 },
  ];
  invokeImpl.handler = async (command, args) => {
    if (fail === command) throw new Error(`boom ${command}`);
    switch (command) {
      case "list_worker_agents":
        return agents;
      case "get_worker_agent":
        return {
          profileId: "web_search",
          origin: "builtin",
          enabled: true,
          pinnedOffer: false,
          revisions: [agents[0]!.draft, agents[0]!.current],
        };
      case "list_worker_tasks":
        return tasks;
      case "list_worker_url_blocklist":
        return blocklist;
      case "set_worker_web_search_mode":
        return (args as { mode: string }).mode;
      case "remove_worker_url_blocklist":
        blocklist = [];
        return true;
      case "set_worker_agent_enabled":
        agents = [{ ...agents[0]!, enabled: (args as { enabled: boolean }).enabled }];
        return agents[0];
      case "approve_worker_agent_revision":
        agents = [{ ...agents[0]!, current: agents[0]!.draft, draft: null }];
        return agents[0];
      default:
        return undefined;
    }
  };
});

let root: Root | null = null;
let restore: (() => void) | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  restore?.();
  restore = null;
});

async function renderPage(language = "ja") {
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  await i18n.changeLanguage(language);
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(WorkerAgentsPage)));
  await settle();
}

async function settle() {
  await act(async () => {
    for (let i = 0; i < 25; i += 1) await Promise.resolve();
  });
}

const button = (label: string) =>
  [...document.querySelectorAll("button")].find(
    (node) => node.textContent === label || node.getAttribute("aria-label") === label,
  )!;
const calls = (command: string) => invokeCalls.filter((call) => call.command === command);
const click = async (node: Element) => {
  await act(async () => (node as HTMLElement).click());
  await settle();
};

test("renders agents, current and draft revisions, tasks and blocklist from the backend", async () => {
  await renderPage();
  const card = document.querySelector("[data-profile=web_search]")!;
  expect(card.textContent).toContain("標準");
  expect(card.textContent).toContain("ウェブを調べる");
  expect(card.textContent).toContain("web_search, fetch_content");
  expect(card.textContent).toContain("新しい目的");
  expect(document.querySelector("[data-task=task-fail]")!.textContent).toContain("no_safe_sources");
  expect(document.querySelector("[data-url-hash=uh-1]")!.textContent).toContain("evil.example");
  expect(document.body.textContent).toContain("削除するまで永続的");
  expect(document.querySelector("[data-mode]")!.getAttribute("data-mode")).toBe("unknown");
});

test("toggling enabled calls set_worker_agent_enabled and refreshes the list", async () => {
  await renderPage();
  const before = calls("list_worker_agents").length;
  const toggle = document.querySelector<HTMLInputElement>("input[role=switch]")!;
  expect(toggle.checked).toBe(true);
  await click(toggle);
  expect(calls("set_worker_agent_enabled")[0]!.args).toEqual({
    profileId: "web_search",
    enabled: false,
  });
  expect(calls("list_worker_agents").length).toBe(before + 1);
  expect(document.querySelector<HTMLInputElement>("input[role=switch]")!.checked).toBe(false);
});

test("approving a draft needs confirmation and sends the draft's hash", async () => {
  await renderPage();
  await click(button("draft を承認"));
  expect(calls("approve_worker_agent_revision")).toHaveLength(0);
  const dialog = document.querySelector("[role=alertdialog]")!;
  expect(dialog.textContent).toContain("新しい目的");
  expect(dialog.textContent).toContain("hash-2");
  await click(button("キャンセル"));
  expect(document.querySelector("[role=alertdialog]")).toBeNull();
  expect(calls("approve_worker_agent_revision")).toHaveLength(0);
  await click(button("draft を承認"));
  await click(button("承認する"));
  expect(calls("approve_worker_agent_revision")[0]!.args).toEqual({
    profileId: "web_search",
    revisionId: "rev-2",
    definitionHash: "hash-2",
  });
  const after = document.querySelector("[data-profile=web_search]")!;
  expect(after.textContent).toContain("現行 (revision 2)");
  expect(after.textContent).not.toContain("draft を承認");
});

test("the revision history lists every revision with its review state", async () => {
  await renderPage();
  await click(button("履歴を表示"));
  expect(calls("get_worker_agent")[0]!.args).toEqual({ profileId: "web_search" });
  const rows = [...document.querySelectorAll("[data-review-state]")].map((row) =>
    row.getAttribute("data-review-state"),
  );
  expect(rows).toEqual(["draft", "approved"]);
});

test("removing a blocklist entry calls remove_worker_url_blocklist and refreshes", async () => {
  await renderPage();
  await click(button("ブロックを解除: evil.example"));
  expect(calls("remove_worker_url_blocklist")[0]!.args).toEqual({ urlHash: "uh-1" });
  expect(document.querySelector("[data-url-hash=uh-1]")).toBeNull();
  expect(document.body.textContent).toContain("ブロックされた URL はありません");
});

test("the web search mode switch sends the chosen mode and shows it", async () => {
  await renderPage();
  await click(button("worker"));
  expect(calls("set_worker_web_search_mode")[0]!.args).toEqual({ mode: "worker" });
  expect(document.querySelector("[data-mode]")!.getAttribute("data-mode")).toBe("worker");
  expect(button("worker").getAttribute("aria-checked")).toBe("true");
  expect(button("inline").getAttribute("aria-checked")).toBe("false");
});

test("only active tasks can be cancelled", async () => {
  await renderPage();
  expect(document.querySelector("[data-task=task-fail] button")).toBeNull();
  await click(button("タスクを取消: task-run"));
  expect(calls("cancel_worker_task")[0]!.args).toEqual({ taskId: "task-run" });
});

test("an invoke rejection is shown as an error and later mutations clear it", async () => {
  fail = "list_worker_tasks";
  await renderPage();
  expect(document.querySelector("[role=alert]")!.textContent).toContain("boom list_worker_tasks");
  fail = "set_worker_web_search_mode";
  await click(button("inline"));
  expect(document.querySelector("[role=alert]")!.textContent).toContain(
    "boom set_worker_web_search_mode",
  );
  expect(document.querySelector("[data-mode]")!.getAttribute("data-mode")).toBe("unknown");
  fail = null;
  await click(button("inline"));
  expect(document.querySelector("[role=alert]")).toBeNull();
});

test("a new draft is saved with defaults, and json_v1 needs a valid output schema", async () => {
  await renderPage();
  await click(button("新しい draft を作成"));
  const set = async (node: Element, value: string) => {
    const proto =
      node instanceof window.HTMLTextAreaElement
        ? window.HTMLTextAreaElement.prototype
        : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(node, value);
    await act(async () => {
      node.dispatchEvent(new window.Event("input", { bubbles: true }));
    });
  };
  const form = document.querySelector("form")!;
  await set(form.querySelector("input")!, "my_worker");
  const areas = form.querySelectorAll("textarea");
  await set(areas[0]!, "調べる");
  await set(areas[1]!, "context");
  await click(form.querySelector("input[type=checkbox]")!);
  const select = form.querySelector("select")!;
  Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, "value")!.set!.call(
    select,
    "json_v1",
  );
  await act(async () => {
    select.dispatchEvent(new window.Event("change", { bubbles: true }));
  });
  await click(button("draft を保存"));
  expect(document.body.textContent).toContain("JSON として読み取れません");
  expect(calls("save_worker_agent_draft")).toHaveLength(0);
  await set(form.querySelectorAll("textarea")[3]!, '{"type":"object"}');
  await click(button("draft を保存"));
  const draft = (calls("save_worker_agent_draft")[0]!.args as { draft: Record<string, unknown> })
    .draft;
  expect(draft).toMatchObject({
    profileId: "my_worker",
    purpose: "調べる",
    systemContext: "context",
    tools: [{ kind: "builtin", key: "web_search", catalogRevisionId: null }],
    outputKind: "json_v1",
    outputSchema: { type: "object" },
    tierPolicy: { maxTier: "local", cloud: "never" },
  });
});

test("english labels follow the app language", async () => {
  await renderPage("en");
  expect(button("Approve draft")).toBeTruthy();
  expect(document.body.textContent).toContain("Recent tasks");
  await act(async () => {
    await i18n.changeLanguage("ja");
  });
});
