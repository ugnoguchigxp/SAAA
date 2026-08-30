export type WebSearchToolCall = {
  id: string;
  type: "function";
  function: { name: "web_search"; arguments: string };
};

const MAX_TOOL_CALL_ID_LENGTH = 160;
const MAX_SEARCH_QUERY_LENGTH = 500;

function hasExactKeys(value: Record<string, unknown>, expected: string[]): boolean {
  return Object.keys(value).sort().join(",") === [...expected].sort().join(",");
}

export function normalizeEndpointBaseUrl(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("evaluation endpoint must be a valid URL");
  }
  if (!["http:", "https:"].includes(url.protocol)) {
    throw new Error("evaluation endpoint must use HTTP or HTTPS");
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("evaluation endpoint must not contain credentials, a query, or a fragment");
  }
  url.pathname = url.pathname.replace(/\/+$/, "");
  return url.toString().replace(/\/$/, "");
}

export function parseWebSearchToolCall(value: unknown): WebSearchToolCall {
  if (!Array.isArray(value) || value.length !== 1) {
    throw new Error("completion must return exactly one web_search tool call");
  }
  const rawCall = value[0];
  if (!rawCall || typeof rawCall !== "object") throw new Error("tool call must be an object");
  const call = rawCall as Record<string, unknown>;
  if (!hasExactKeys(call, ["id", "type", "function"])) throw new Error("tool call has an invalid shape");
  if (typeof call.id !== "string" || !call.id.trim() || call.id.length > MAX_TOOL_CALL_ID_LENGTH) {
    throw new Error("tool call id is invalid");
  }
  if (call.type !== "function" || !call.function || typeof call.function !== "object") {
    throw new Error("tool call must invoke a function");
  }
  const fn = call.function as Record<string, unknown>;
  if (!hasExactKeys(fn, ["name", "arguments"]) || fn.name !== "web_search" || typeof fn.arguments !== "string") {
    throw new Error("tool call must invoke web_search with JSON arguments");
  }
  let args: unknown;
  try {
    args = JSON.parse(fn.arguments);
  } catch {
    throw new Error("web_search arguments must be strict JSON");
  }
  if (!args || typeof args !== "object" || !hasExactKeys(args as Record<string, unknown>, ["query"])) {
    throw new Error("web_search arguments must contain only query");
  }
  const query = (args as Record<string, unknown>).query;
  if (typeof query !== "string" || !query.trim() || query.length > MAX_SEARCH_QUERY_LENGTH) {
    throw new Error("web_search query is invalid");
  }
  return call as WebSearchToolCall;
}
