import { expect, mock, test } from "bun:test";

let captured: unknown;
mock.module("@openai/codex-sdk", () => ({
  Codex: class {
    constructor(config: unknown) {
      captured = config;
    }
  },
}));

const { isolatedCodex } = await import("../scripts/role-routing/codex-isolation");

test("role-routing Codex isolation permits only the explicit loopback gateway", () => {
  isolatedCodex("http://127.0.0.1:43127/mcp?rrRoot=root");
  const value = captured as {
    env: Record<string, string>;
    config: { mcp_servers: Record<string, { url: string; bearer_token_env_var: string }>; web_search: string; sandbox_workspace_write: { network_access: boolean } };
  };
  expect(value.env.OPENAI_API_KEY).toBeUndefined();
  expect(value.config.web_search).toBe("disabled");
  expect(value.config.sandbox_workspace_write.network_access).toBe(false);
  expect(value.config.mcp_servers).toEqual({
    "saaa-role-routing": {
      url: "http://127.0.0.1:43127/mcp?rrRoot=root",
      bearer_token_env_var: "SAAA_ROLE_ROUTING_MCP_TOKEN",
    },
  });
});

test("role-routing Codex has no inherited MCP server without a gateway", () => {
  isolatedCodex();
  const value = captured as { config: { mcp_servers: Record<string, unknown> } };
  expect(value.config.mcp_servers).toEqual({});
});
