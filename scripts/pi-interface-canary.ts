import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHash } from "node:crypto";
import { PiProbeClient, dataOf } from "./pi-interface/rpc-client";
import { startFixtureProvider } from "./pi-interface/fixture-provider";

export async function runPiCanary(binary: string) {
  const version = Bun.spawnSync([binary, "--version"]);
  if (version.exitCode !== 0 || version.stdout.toString().trim() !== "0.86.1")
    throw new Error("unsupported-pi-version: expected 0.86.1");
  const root = mkdtempSync(join(tmpdir(), "saaa-pi-canary-"));
  const workspace = join(root, "workspace");
  const agentDir = join(root, "agent");
  mkdirSync(workspace);
  mkdirSync(agentDir);
  if (Bun.spawnSync(["git", "init", "-q", workspace]).exitCode !== 0)
    throw new Error("fixture-git-init-failed");
  const provider = startFixtureProvider();
  writeFileSync(
    join(agentDir, "models.json"),
    JSON.stringify({
      providers: {
        "saaa-fixture": {
          baseUrl: provider.url,
          api: "openai-completions",
          apiKey: "fixture-only",
          models: [
            {
              id: "fixture",
              reasoning: false,
              input: ["text"],
              contextWindow: 128000,
              maxTokens: 4096,
            },
          ],
        },
      },
    }),
  );
  writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ retry: { enabled: false } }));
  const session = join(root, "session.jsonl");
  // Only these variables are inherited; no real provider credentials or user resources.
  const env = {
    PATH: process.env.PATH,
    HOME: root,
    PI_CODING_AGENT_DIR: agentDir,
    PI_SKIP_VERSION_CHECK: "1",
    NO_COLOR: "1",
  };
  const args = [
    "--mode",
    "rpc",
    "--session",
    session,
    "--provider",
    "saaa-fixture",
    "--model",
    "fixture",
    "--no-extensions",
    "--no-skills",
    "--no-prompt-templates",
    "--no-context-files",
    "--no-approve",
  ];
  const pids: number[] = [];
  const open = async () => {
    const client = new PiProbeClient(binary, args, workspace, env);
    if (client.process.pid) pids.push(client.process.pid);
    try {
      const state = dataOf(await client.command("get_state"));
      if (state.sessionFile !== session) throw new Error("session-path-mismatch");
      return { client, state };
    } catch (error) {
      await client.close();
      throw error;
    }
  };
  const execute = async (prompt: string) => {
    const { client, state } = await open();
    try {
      const before = dataOf(await client.command("get_entries"));
      const start = client.events.length;
      const response = await client.command("prompt", { message: prompt });
      if (response.success !== true) throw new Error("prompt-rejected");
      await client.waitFor((event) => event.type === "agent_settled", start);
      const entries = dataOf(
        await client.command("get_entries", before.leafId ? { since: before.leafId } : {}),
      );
      if (!Array.isArray(entries.entries) || entries.entries.length === 0)
        throw new Error("missing-result-entries");
      return {
        sessionId: state.sessionId,
        events: client.events.slice(start),
        entries: entries.entries,
      };
    } finally {
      await client.close();
    }
  };
  try {
    const first = await execute("probe-first: result.txtへfirstを書いてください。");
    if (readFileSync(join(workspace, "result.txt"), "utf8") !== "first\n")
      throw new Error("first-write-failed");
    const second = await execute(
      "probe-resume: 前の作業を引き継ぎresult.txtをsecondへ変更してください。",
    );
    if (first.sessionId !== second.sessionId || !provider.stats().resumed)
      throw new Error("session-resume-failed");
    if (readFileSync(join(workspace, "result.txt"), "utf8") !== "second\n")
      throw new Error("second-write-failed");
    const failed = await execute("probe-error");
    if (
      !failed.events.some(
        (event) =>
          event.type === "message_end" &&
          (event.message as Record<string, unknown>)?.stopReason === "error",
      )
    )
      throw new Error("model-error-not-observed");
    const { client } = await open();
    try {
      const start = client.events.length;
      await client.command("prompt", { message: "probe-wait" });
      await client.waitFor((event) => event.type === "tool_execution_start", start);
      if ((await client.command("clear_queue")).success !== true)
        throw new Error("clear-queue-failed");
      if ((await client.command("abort")).success !== true) throw new Error("abort-failed");
      await client.waitFor((event) => event.type === "agent_settled", start);
    } finally {
      await client.close();
    }
    const lines = readFileSync(session, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    const report = {
      schemaVersion: 1,
      status: "passed",
      profile: "real-pi-scripted-provider",
      root,
      piVersion: "0.86.1",
      binary: realpathSync(binary),
      binarySha256: createHash("sha256")
        .update(readFileSync(realpathSync(binary)))
        .digest("hex"),
      sessionId: first.sessionId,
      processIds: pids,
      sessionEntries: lines.length,
      checks: {
        firstWrite: true,
        sameSessionAfterExit: true,
        historyReceivedByProvider: true,
        secondWrite: true,
        modelError: true,
        abort: true,
        normalShutdown: true,
      },
      provider: provider.stats(),
      liveModel: "not-run",
      recordedAt: new Date().toISOString(),
    };
    writeFileSync(join(root, "report.json"), JSON.stringify(report, null, 2) + "\n");
    return report;
  } finally {
    provider.stop();
  }
}

if (import.meta.main) {
  const binary = process.env.SAAA_PI_BINARY ?? Bun.which("pi");
  if (!binary) throw new Error("Set SAAA_PI_BINARY to the pinned pi executable path");
  console.log(JSON.stringify(await runPiCanary(resolve(binary)), null, 2));
}
