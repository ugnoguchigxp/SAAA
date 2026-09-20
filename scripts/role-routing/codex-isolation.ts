import { Codex } from "@openai/codex-sdk";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function environment(): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SSL_CERT_FILE", "SSL_CERT_DIR"]) {
    const value = process.env[key];
    if (value) result[key] = value;
  }
  return result;
}

export function isolatedCodex(): Codex {
  return new Codex({
    env: environment(),
    config: {
      mcp_servers: {},
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
