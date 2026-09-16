import { Codex } from "@openai/codex-sdk";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Explicit live canary: only this temporary Git workspace is delegated for edits.
const workspace = mkdtempSync(join(tmpdir(), "saaa-codex-canary-"));
if (Bun.spawnSync(["git", "init", "-q", workspace]).exitCode !== 0)
  throw new Error("git-init-failed");
const model = "gpt-5.6-luna";
const options = {
  model,
  workingDirectory: workspace,
  sandboxMode: "workspace-write" as const,
  approvalPolicy: "never" as const,
  networkAccessEnabled: false,
  webSearchMode: "disabled" as const,
  modelReasoningEffort: "low" as const,
};
const codexOptions = { config: { mcp_servers: {} } };
const first = new Codex(codexOptions).startThread(options);
await first.run(
  "Create result.txt containing exactly first followed by a newline. Remember the marker SAAA_RESUME_7391 for the next turn. Do not access anything outside this workspace.",
  { signal: AbortSignal.timeout(120_000) },
);
if (readFileSync(join(workspace, "result.txt"), "utf8") !== "first\n")
  throw new Error("first-write-failed");
if (!first.id) throw new Error("missing-thread-id");
const second = new Codex(codexOptions).resumeThread(first.id, options);
await second.run(
  "Replace result.txt with the marker I asked you to remember, followed by a newline. Do not access anything outside this workspace.",
  { signal: AbortSignal.timeout(120_000) },
);
if (readFileSync(join(workspace, "result.txt"), "utf8") !== "SAAA_RESUME_7391\n")
  throw new Error("resume-failed");
console.log(
  JSON.stringify(
    {
      status: "passed",
      model,
      workspace,
      threadId: first.id,
      checks: { edit: true, sdkRecreated: true, sessionResumed: true, rememberedContext: true },
      recordedAt: new Date().toISOString(),
    },
    null,
    2,
  ),
);
