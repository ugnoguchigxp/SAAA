import { Codex } from "@openai/codex-sdk";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function environment(includeBridgeToken: boolean): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SSL_CERT_FILE", "SSL_CERT_DIR"]) {
    const value = process.env[key];
    if (value) result[key] = value;
  }
  if (includeBridgeToken) {
    const bridgeToken = process.env.SAAA_ROLE_ROUTING_MCP_TOKEN;
    if (bridgeToken) result.SAAA_ROLE_ROUTING_MCP_TOKEN = bridgeToken;
  }
  return result;
}

export function isolatedCodex(toolGatewayUrl?: string): Codex {
  const mcpServers = toolGatewayUrl
    ? {
        "saaa-role-routing": {
          url: toolGatewayUrl,
          bearer_token_env_var: "SAAA_ROLE_ROUTING_MCP_TOKEN",
        },
      }
    : {};
  return new Codex({
    env: environment(Boolean(toolGatewayUrl)),
    config: {
      mcp_servers: mcpServers,
      web_search: "disabled",
      sandbox_workspace_write: { network_access: false },
    },
  });
}

export function isolatedWorkingDirectory(): string {
  return mkdtempSync(join(tmpdir(), "saaa-role-routing-"));
}

export function removeIsolatedWorkingDirectory(path: string | undefined) {
  if (path) rmSync(path, { recursive: true, force: true });
}
