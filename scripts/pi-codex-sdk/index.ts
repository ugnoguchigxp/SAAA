import { Codex, type ThreadItem } from "@openai/codex-sdk";
import { createAssistantMessageEventStream, type AssistantMessage } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

const PROVIDER = "saaa-codex-sdk";
const MODEL = "gpt-5.6-luna";
const BINDING = "saaa.codex-sdk.binding";
const OBSERVATION = "saaa.codex-sdk.observation";

function observation(item: ThreadItem) {
  switch (item.type) {
    case "command_execution":
      return {
        kind: item.type,
        command: item.command.slice(0, 1000),
        exitCode: item.exit_code,
        isError: item.status === "failed" || (item.exit_code !== undefined && item.exit_code !== 0),
      };
    case "file_change":
      return {
        kind: item.type,
        changes: item.changes
          .slice(0, 30)
          .map((change) => ({ path: change.path.slice(0, 500), kind: change.kind })),
        isError: item.status === "failed",
      };
    case "mcp_tool_call":
      return {
        kind: item.type,
        tool: item.tool.slice(0, 100),
        server: item.server.slice(0, 100),
        isError: item.status === "failed",
      };
    default:
      return undefined;
  }
}

// This is an explicit SAAA resource profile. No credentials are imported into pi.
export default async function (pi: ExtensionAPI) {
  const login = await pi.exec("codex", ["login", "status"], { timeout: 5000 });
  if (login.code !== 0) throw new Error("Codex SDK requires an existing Codex login");
  const configured = await pi.exec("codex", ["mcp", "list", "--json"], { timeout: 5000 });
  if (configured.code !== 0) throw new Error("Codex SDK configuration unavailable");
  const servers: unknown = JSON.parse(configured.stdout);
  if (!Array.isArray(servers)) throw new Error("Invalid Codex MCP configuration");
  const disabledServers = Object.fromEntries(
    servers.map((server: { name: unknown; transport?: { type?: string } }) => {
      if (typeof server.name !== "string") throw new Error("Invalid Codex MCP server name");
      const transport =
        server.transport?.type === "stdio"
          ? { command: "saaa-disabled-mcp" }
          : { url: "http://127.0.0.1:1/saaa-disabled-mcp" };
      return [server.name, { enabled: false, ...transport }];
    }),
  );
  let session: ExtensionContext | undefined;
  let threadId: string | undefined;
  let active = false;
  let bindingError = false;
  pi.on("session_start", (_event, ctx) => {
    session = ctx;
    threadId = undefined;
    bindingError = false;
    for (const entry of ctx.sessionManager.getEntries()) {
      if (entry.type !== "custom" || entry.customType !== BINDING) continue;
      const data = entry.data as { threadId?: unknown; cwd?: unknown; model?: unknown };
      if (
        !data ||
        data.cwd !== ctx.cwd ||
        data.model !== MODEL ||
        typeof data.threadId !== "string"
      ) {
        bindingError = true;
        return;
      }
      threadId = data.threadId;
    }
    // The SDK owns implementation tools; pi owns the RPC/session lifecycle.
    pi.setActiveTools([]);
  });
  pi.registerProvider(PROVIDER, {
    baseUrl: "http://localhost.invalid",
    apiKey: "local-sdk-auth-managed-by-codex",
    api: PROVIDER,
    models: [
      {
        id: MODEL,
        name: `${MODEL} (Codex SDK)`,
        reasoning: true,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 128000,
        maxTokens: 16000,
      },
    ],
    streamSimple(model, context, options) {
      const stream = createAssistantMessageEventStream();
      const output: AssistantMessage = {
        role: "assistant",
        api: model.api,
        provider: model.provider,
        model: model.id,
        content: [],
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason: "pending",
        timestamp: Date.now(),
      };
      void (async () => {
        const controller = new AbortController();
        let claimed = false;
        try {
          if (bindingError) throw new Error("Codex SDK session binding mismatch");
          if (!session || active) throw new Error("Codex SDK session is unavailable or busy");
          active = true;
          claimed = true;
          const current = session;
          const user = context.messages.filter((message) => message.role === "user").at(-1);
          if (!user) throw new Error("Missing user request");
          const prompt =
            typeof user.content === "string"
              ? user.content
              : user.content
                  .filter((part) => part.type === "text")
                  .map((part) => part.text)
                  .join("\n");
          if (!prompt.trim() || [...prompt].length > 32000)
            throw new Error("Invalid coding request");
          const client = new Codex({ config: { mcp_servers: disabledServers } });
          const configuration = {
            model: MODEL,
            workingDirectory: current.cwd,
            sandboxMode: "read-only" as const,
            approvalPolicy: "never" as const,
            networkAccessEnabled: false,
            webSearchMode: "disabled" as const,
            modelReasoningEffort: "low" as const,
          };
          const thread = threadId
            ? client.resumeThread(threadId, configuration)
            : client.startThread(configuration);
          const signal = options?.signal
            ? AbortSignal.any([options.signal, controller.signal])
            : controller.signal;
          stream.push({ type: "start", partial: output });
          const { events } = await thread.runStreamed(prompt, { signal });
          let final = "";
          let completed = false;
          let bytes = 0;
          let observations = 0;
          for await (const event of events) {
            bytes += Buffer.byteLength(JSON.stringify(event));
            if (bytes > 64 * 1024 * 1024) {
              controller.abort();
              throw new Error("Codex SDK output limit");
            }
            if (event.type === "thread.started") {
              threadId = event.thread_id;
              pi.appendEntry(BINDING, { threadId, cwd: current.cwd, model: MODEL });
            } else if (event.type === "item.completed") {
              if (event.item.type === "agent_message") final = event.item.text;
              const record = observation(event.item);
              if (record && observations++ < 100) pi.appendEntry(OBSERVATION, record);
            } else if (event.type === "turn.failed") {
              throw new Error(event.error.message);
            } else if (event.type === "error") {
              throw new Error(event.message);
            } else if (event.type === "turn.completed") {
              completed = true;
              output.usage.input = event.usage.input_tokens;
              output.usage.output = event.usage.output_tokens;
              output.usage.cacheRead = event.usage.cached_input_tokens;
              output.usage.totalTokens = event.usage.input_tokens + event.usage.output_tokens;
            }
          }
          if (!completed || signal.aborted) throw new Error("Codex SDK turn did not complete");
          const text = final || "Execution ended without a final message.";
          output.content = [{ type: "text", text }];
          stream.push({ type: "text_start", contentIndex: 0, partial: output });
          stream.push({
            type: "text_delta",
            contentIndex: 0,
            delta: text,
            partial: output,
          });
          stream.push({
            type: "text_end",
            contentIndex: 0,
            content: text,
            partial: output,
          });
          output.stopReason = "stop";
          stream.push({ type: "done", reason: "stop", message: output });
        } catch (error) {
          output.stopReason = options?.signal?.aborted ? "aborted" : "error";
          output.errorMessage = error instanceof Error ? error.message : "Codex SDK failed";
          stream.push({ type: "error", reason: output.stopReason, error: output });
        } finally {
          controller.abort();
          if (claimed) active = false;
          stream.end();
        }
      })();
      return stream;
    },
  });
}
