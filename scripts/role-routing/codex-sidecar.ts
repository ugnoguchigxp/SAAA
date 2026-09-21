#!/usr/bin/env bun
// Role-routing Codex SDK sidecar. Stdout is JSONL protocol only; diagnostics go to stderr.
import {
  isolatedCodex,
  isolatedWorkingDirectory,
  removeIsolatedWorkingDirectory,
} from "./codex-isolation";

const VERSION = 1;
const MAX_LINE_BYTES = 1024 * 1024;
const MAX_STREAM_BYTES = 8 * 1024 * 1024;
const MAX_FINAL_BYTES = 64 * 1024;
const MAX_ID_BYTES = 160;
const MAX_MODEL_BYTES = 160;
const FIXED_ERROR_CODES = new Set([
  "invalid_request",
  "stream_limit",
  "sdk_error",
  "incomplete_result",
  "invalid_output_schema",
]);

type RunFrame = {
  version: number;
  id: string;
  op: "run";
  stepId: string;
  model: string;
  prompt: string;
  outputSchema?: Record<string, unknown>;
  toolGatewayUrl?: string;
  timeoutMs: number;
};
type CancelFrame = { version: number; id: string; op: "cancel"; stepId: string };
type Frame = RunFrame | CancelFrame;

const active = new Map<string, AbortController>();

function emit(value: Record<string, unknown>) {
  const encoded = JSON.stringify({ version: VERSION, ...value });
  if (Buffer.byteLength(encoded) > MAX_LINE_BYTES) {
    console.error("role-routing sidecar attempted an oversized protocol frame");
    return;
  }
  process.stdout.write(`${encoded}\n`);
}

function fail(frame: Pick<Frame, "id" | "stepId">, code: string) {
  emit({ id: frame.id, stepId: frame.stepId, op: "failed", code });
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const keys = Object.keys(value);
  return keys.length === allowed.length && keys.every((key) => allowed.includes(key));
}

function boundedText(value: unknown, maxBytes: number): value is string {
  return typeof value === "string" && value.length > 0 && Buffer.byteLength(value) <= maxBytes;
}

function valid(frame: unknown): frame is Frame {
  if (!frame || typeof frame !== "object") return false;
  const value = frame as Record<string, unknown>;
  if (
    value.version !== VERSION ||
    !boundedText(value.id, MAX_ID_BYTES) ||
    !boundedText(value.stepId, MAX_ID_BYTES)
  )
    return false;
  if (value.op === "cancel") {
    return exactKeys(value, ["version", "id", "op", "stepId"]);
  }
  const allowed = ["version", "id", "op", "stepId", "model", "prompt", "timeoutMs"];
  if (value.outputSchema !== undefined) allowed.push("outputSchema");
  if (value.toolGatewayUrl !== undefined) allowed.push("toolGatewayUrl");
  return (
    exactKeys(value, allowed) &&
    value.op === "run" &&
    boundedText(value.model, MAX_MODEL_BYTES) &&
    boundedText(value.prompt, MAX_FINAL_BYTES * 8) &&
    typeof value.timeoutMs === "number" &&
    Number.isInteger(value.timeoutMs) &&
    value.timeoutMs >= 1_000 &&
    value.timeoutMs <= 300_000 &&
    (value.outputSchema === undefined ||
      (typeof value.outputSchema === "object" &&
        value.outputSchema !== null &&
        !Array.isArray(value.outputSchema))) &&
    (value.toolGatewayUrl === undefined || validToolGatewayUrl(value.toolGatewayUrl))
  );
}

function validToolGatewayUrl(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 512) return false;
  try {
    const url = new URL(value);
    const roots = url.searchParams.getAll("rrRoot");
    return (
      url.protocol === "http:" &&
      url.hostname === "127.0.0.1" &&
      url.pathname === "/mcp" &&
      roots.length === 1 &&
      roots[0].length > 0 &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}

async function run(frame: RunFrame) {
  if (active.has(frame.stepId) || Buffer.byteLength(frame.prompt) > MAX_FINAL_BYTES * 8) {
    fail(frame, "invalid_request");
    return;
  }
  const controller = new AbortController();
  active.set(frame.stepId, controller);
  const timeout = setTimeout(() => controller.abort(), frame.timeoutMs);
  let workingDirectory: string | undefined;
  try {
    workingDirectory = isolatedWorkingDirectory();
    emit({ id: frame.id, stepId: frame.stepId, op: "started" });
    const thread = isolatedCodex(frame.toolGatewayUrl).startThread({
      model: frame.model,
      sandboxMode: "read-only",
      approvalPolicy: "never",
      networkAccessEnabled: false,
      webSearchMode: "disabled",
      skipGitRepoCheck: true,
      workingDirectory: workingDirectory,
    });
    const { events } = await thread.runStreamed(frame.prompt, {
      signal: controller.signal,
      outputSchema: frame.outputSchema,
    });
    let totalBytes = 0;
    let finalText = "";
    let completed = false;
    let usage: Record<string, number> | undefined;
    for await (const event of events) {
      totalBytes += Buffer.byteLength(JSON.stringify(event));
      if (totalBytes > MAX_STREAM_BYTES) throw new Error("stream_limit");
      if (event.type === "item.completed" && event.item.type === "agent_message") {
        finalText = event.item.text;
      } else if (event.type === "turn.completed") {
        completed = true;
        usage = {
          inputTokens: event.usage.input_tokens,
          cachedInputTokens: event.usage.cached_input_tokens,
          outputTokens: event.usage.output_tokens,
          reasoningOutputTokens: event.usage.reasoning_output_tokens,
        };
      } else if (event.type === "turn.failed" || event.type === "error") {
        throw new Error("sdk_error");
      } else if (event.type === "item.started") {
        emit({ id: frame.id, stepId: frame.stepId, op: "activity" });
      }
    }
    if (controller.signal.aborted) {
      emit({ id: frame.id, stepId: frame.stepId, op: "cancelled" });
      return;
    }
    if (!completed || !finalText || Buffer.byteLength(finalText) > MAX_FINAL_BYTES) {
      throw new Error("incomplete_result");
    }
    if (frame.outputSchema) {
      try {
        JSON.parse(finalText);
      } catch {
        throw new Error("invalid_output_schema");
      }
    }
    emit({ id: frame.id, stepId: frame.stepId, op: "result", text: finalText, usage });
  } catch (error) {
    const requestedCode = error instanceof Error ? error.message : "sdk_error";
    const code = FIXED_ERROR_CODES.has(requestedCode) ? requestedCode : "sdk_error";
    emit({
      id: frame.id,
      stepId: frame.stepId,
      op: controller.signal.aborted ? "cancelled" : "failed",
      ...(controller.signal.aborted ? {} : { code }),
    });
  } finally {
    clearTimeout(timeout);
    active.delete(frame.stepId);
    removeIsolatedWorkingDirectory(workingDirectory);
  }
}

async function receive(line: string) {
  if (Buffer.byteLength(line) > MAX_LINE_BYTES) return;
  let frame: unknown;
  try {
    frame = JSON.parse(line);
  } catch {
    return;
  }
  if (!valid(frame)) return;
  if (frame.op === "cancel") {
    active.get(frame.stepId)?.abort();
    return;
  }
  void run(frame);
}

let buffered = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk: string) => {
  buffered += chunk;
  while (true) {
    const boundary = buffered.indexOf("\n");
    if (boundary < 0) break;
    const line = buffered.slice(0, boundary);
    buffered = buffered.slice(boundary + 1);
    void receive(line);
  }
  if (Buffer.byteLength(buffered) > MAX_LINE_BYTES) buffered = "";
});
