type ChatMessage = { role: string; content?: unknown; tool_calls?: unknown[] };

// Actual pi model transport and built-in tools run against this local server.
// It is intentionally not an LLM and cannot establish coding quality.
export function startFixtureProvider() {
  let requests = 0;
  let resumed = false;
  let waiting = false;
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      if (new URL(request.url).pathname !== "/v1/chat/completions")
        return new Response("not-found", { status: 404 });
      const input = (await request.json()) as { messages?: ChatMessage[] };
      const messages = input.messages ?? [];
      const user = messages.findLast((message) => message.role === "user");
      const prompt = JSON.stringify(user?.content);
      requests += 1;
      if (prompt.includes("probe-error"))
        return Response.json({ error: { message: "deterministic failure" } }, { status: 400 });
      const last = messages.at(-1);
      if (prompt.includes("probe-resume"))
        resumed = messages.some(
          (message) =>
            message.role === "assistant" && JSON.stringify(message).includes("first-done"),
        );
      const tool = last?.role !== "tool";
      let delta: Record<string, unknown>;
      if (tool && prompt.includes("probe-wait")) {
        waiting = true;
        delta = {
          tool_calls: [
            {
              index: 0,
              id: "wait-tool",
              type: "function",
              function: {
                name: "bash",
                arguments: JSON.stringify({ command: "sleep 60" }),
              },
            },
          ],
        };
      } else if (tool) {
        const content = prompt.includes("probe-resume") ? "second\n" : "first\n";
        delta = {
          tool_calls: [
            {
              index: 0,
              id: `write-${requests}`,
              type: "function",
              function: {
                name: "write",
                arguments: JSON.stringify({ path: "result.txt", content }),
              },
            },
          ],
        };
      } else {
        delta = { content: prompt.includes("probe-resume") ? "second-done" : "first-done" };
      }
      const chunk = (body: Record<string, unknown>, finish: string | null) =>
        `data: ${JSON.stringify({
          id: `fixture-${requests}`,
          object: "chat.completion.chunk",
          created: 1,
          model: "fixture",
          choices: [{ index: 0, delta: body, finish_reason: finish }],
        })}\n\n`;
      return new Response(
        chunk({ role: "assistant", ...delta }, null) +
          chunk({}, tool ? "tool_calls" : "stop") +
          "data: [DONE]\n\n",
        { headers: { "content-type": "text/event-stream" } },
      );
    },
  });
  return {
    url: `http://127.0.0.1:${server.port}/v1`,
    stats: () => ({ requests, resumed, waiting }),
    stop: () => server.stop(true),
  };
}
