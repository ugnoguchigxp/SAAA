import { describe, expect, mock, test } from "bun:test";

let events: unknown[] = [];
let resumed: string | undefined;
let capturedPrompt = "";
mock.module("@openai/codex-sdk", () => ({
  Codex: class {
    startThread() {
      resumed = undefined;
      return this.thread();
    }
    resumeThread(id: string) {
      const thread = this.thread();
      resumed = id;
      return thread;
    }
    thread() {
      return {
        runStreamed: async (prompt: string) => {
          capturedPrompt = prompt;
          return {
            events: (async function* () {
              for (const event of events) yield event;
            })(),
          };
        },
      };
    }
  },
}));
mock.module("@earendil-works/pi-ai", () => ({
  createAssistantMessageEventStream() {
    const values: unknown[] = [];
    let end!: () => void;
    const finished = new Promise<void>((resolve) => {
      end = resolve;
    });
    return { values, finished, push: (value: unknown) => values.push(value), end };
  },
}));
const { default: extension } = await import("../scripts/pi-codex-sdk/index");
async function setup(entries: unknown[] = []) {
  let start: (event: unknown, context: unknown) => void = () => {};
  let provider: any;
  const records: any[] = [];
  let tools: string[] | undefined;
  await extension({
    exec: async () => ({ code: 0, stdout: "[]" }),
    on: (_name: string, handler: typeof start) => {
      start = handler;
    },
    setActiveTools: (names: string[]) => {
      tools = names;
    },
    appendEntry: (customType: string, data: unknown) => records.push({ customType, data }),
    registerProvider: (_name: string, config: unknown) => {
      provider = config;
    },
  } as any);
  start({}, { cwd: "/workspace", sessionManager: { getEntries: () => entries } });
  return {
    records,
    tools,
    run: async () => {
      const stream = provider.streamSimple(
        { api: "saaa-codex-sdk", provider: "saaa-codex-sdk", id: "gpt-5.6-luna" },
        {
          messages: [
            { role: "user", content: "old" },
            { role: "assistant", content: [] },
            { role: "user", content: "new request" },
          ],
        },
        {},
      );
      await stream.finished;
      return stream.values as any[];
    },
  };
}
describe("pi Codex SDK provider", () => {
  test("persists binding and bounded observations and resumes the SDK task", async () => {
    events = [
      { type: "thread.started", thread_id: "task-1" },
      {
        type: "item.completed",
        item: {
          type: "command_execution",
          command: "x".repeat(2000),
          status: "completed",
          exit_code: 0,
        },
      },
      { type: "item.completed", item: { type: "agent_message", text: "done" } },
      {
        type: "turn.completed",
        usage: { input_tokens: 2, output_tokens: 1, cached_input_tokens: 0 },
      },
    ];
    const first = await setup();
    expect(first.tools).toEqual([]);
    expect((await first.run()).at(-1).type).toBe("done");
    expect(capturedPrompt).toBe("new request");
    expect(first.records[1].data.command.length).toBe(1000);
    const next = await setup([{ type: "custom", ...first.records[0] }]);
    await next.run();
    expect(resumed).toBe("task-1");
  });
  test("rejects mismatched workspace bindings without executing", async () => {
    capturedPrompt = "";
    const provider = await setup([
      {
        type: "custom",
        customType: "saaa.codex-sdk.binding",
        data: { cwd: "/other", model: "gpt-5.6-luna", threadId: "task-1" },
      },
    ]);
    expect((await provider.run()).at(-1).type).toBe("error");
    expect(capturedPrompt).toBe("");
  });
  test("does not report success on an incomplete SDK stream", async () => {
    events = [{ type: "thread.started", thread_id: "task-2" }];
    const provider = await setup();
    expect((await provider.run()).at(-1).type).toBe("error");
  });
});
