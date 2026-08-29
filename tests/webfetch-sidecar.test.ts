import { describe, expect, test } from "bun:test";
import { LlmFetchError, createLlmFetch, duckDuckGo } from "llm-fetch";
import {
  WEB_FETCH_TOOL_NAMES,
  parseInvocation,
  safeFailure,
} from "../scripts/webfetch-sidecar";

describe("WebFetch sidecar protocol", () => {
  test("accepts only the two model-facing llm-fetch tools", () => {
    expect(parseInvocation('{"name":"web_search","arguments":{"query":"SAAA","limit":5}}')).toEqual({
      name: "web_search",
      arguments: { query: "SAAA", limit: 5 },
    });
    expect(parseInvocation('{"name":"fetch_content","arguments":{"url":"https://example.com","maxCharacters":5000}}').name).toBe("fetch_content");
    expect(() => parseInvocation('{"name":"shell","arguments":{}}')).toThrow();
    expect(() => parseInvocation('{"name":"web_search","arguments":{},"extra":true}')).toThrow();
  });

  test("uses the package's strict OpenAI Chat Completions definitions", async () => {
    const web = createLlmFetch({ search: duckDuckGo(), contextGuard: { profile: "strict" } });
    try {
      const definitions = web.toolset().openaiChatCompletionsDefinitions();
      expect(definitions.map((definition) => definition.function.name)).toEqual(WEB_FETCH_TOOL_NAMES);
      expect(definitions.every((definition) => definition.function.strict)).toBe(true);
      expect(definitions.every((definition) => definition.function.parameters.additionalProperties === false)).toBe(true);
    } finally {
      await web.close();
    }
  });

  test("projects typed failures and redacts unknown exceptions", () => {
    expect(safeFailure(new LlmFetchError("UNSAFE_URL", "Local destinations are blocked."))).toEqual({
      ok: false,
      error: {
        code: "UNSAFE_URL",
        message: "Local destinations are blocked.",
        retryable: false,
      },
    });
    expect(safeFailure(new Error("secret detail"))).toEqual({
      ok: false,
      error: {
        code: "UNKNOWN",
        message: "WebFetch could not complete the request.",
        retryable: false,
      },
    });
  });
});
