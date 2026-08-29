import {
  LlmFetchError,
  brave,
  createLlmFetch,
  duckDuckGo,
  fallbackSearch,
  type SearchProvider,
  type ToolExecutionResult,
} from "llm-fetch";

export const WEB_FETCH_TOOL_NAMES = ["web_search", "fetch_content"] as const;

type WebFetchToolName = (typeof WEB_FETCH_TOOL_NAMES)[number];

export type WebFetchInvocation = {
  name: WebFetchToolName;
  arguments: Record<string, unknown>;
};

type WebFetchFailure = {
  ok: false;
  error: {
    code: string;
    message: string;
    retryable: boolean;
    guardDecision?: string;
    warningCategories?: readonly string[];
  };
};

type WebFetchResponse = { ok: true; result: ToolExecutionResult } | WebFetchFailure;

const MAX_INPUT_BYTES = 16 * 1024;

export function parseInvocation(raw: string): WebFetchInvocation {
  if (Buffer.byteLength(raw) > MAX_INPUT_BYTES) {
    throw new LlmFetchError("RESPONSE_TOO_LARGE", "WebFetch invocation is too large.");
  }
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new LlmFetchError("INVALID_INPUT", "WebFetch invocation must be valid JSON.");
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new LlmFetchError("INVALID_INPUT", "WebFetch invocation must be an object.");
  }
  const record = value as Record<string, unknown>;
  if (
    Object.keys(record).some((key) => key !== "name" && key !== "arguments")
    || !WEB_FETCH_TOOL_NAMES.includes(record.name as WebFetchToolName)
    || !record.arguments
    || typeof record.arguments !== "object"
    || Array.isArray(record.arguments)
  ) {
    throw new LlmFetchError("INVALID_INPUT", "WebFetch invocation does not match the tool protocol.");
  }
  return {
    name: record.name as WebFetchToolName,
    arguments: record.arguments as Record<string, unknown>,
  };
}

export function safeFailure(error: unknown): WebFetchFailure {
  if (error instanceof LlmFetchError) {
    return {
      ok: false,
      error: {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        ...(error.guardDecision ? { guardDecision: error.guardDecision } : {}),
        ...(error.warningCategories ? { warningCategories: error.warningCategories } : {}),
      },
    };
  }
  return {
    ok: false,
    error: {
      code: "UNKNOWN",
      message: "WebFetch could not complete the request.",
      retryable: false,
    },
  };
}

function searchProvider(environment: NodeJS.ProcessEnv): SearchProvider {
  const duckDuckGoProvider = duckDuckGo();
  const braveApiKey = environment.BRAVE_SEARCH_API_KEY?.trim();
  if (!braveApiKey) return duckDuckGoProvider;
  return fallbackSearch([duckDuckGoProvider, brave({ apiKey: braveApiKey })]);
}

export async function executeInvocation(
  invocation: WebFetchInvocation,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<WebFetchResponse> {
  const web = createLlmFetch({
    search: searchProvider(environment),
    contextGuard: { profile: "strict" },
  });
  try {
    return {
      ok: true,
      result: await web.toolset().execute(invocation.name, invocation.arguments),
    };
  } catch (error) {
    return safeFailure(error);
  } finally {
    await web.close().catch(() => undefined);
  }
}

async function readInput(): Promise<string> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += buffer.length;
    if (size > MAX_INPUT_BYTES) {
      throw new LlmFetchError("RESPONSE_TOO_LARGE", "WebFetch invocation is too large.");
    }
    chunks.push(buffer);
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function main(): Promise<void> {
  let response: WebFetchResponse;
  try {
    response = await executeInvocation(parseInvocation(await readInput()));
  } catch (error) {
    response = safeFailure(error);
  }
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

if (import.meta.main) {
  await main();
}
