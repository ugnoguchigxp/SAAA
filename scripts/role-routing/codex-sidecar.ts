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

type RunFrame = {
  version: number;
  id: string;
  op: "run";
  stepId: string;
  model: string;
  prompt: string;
  outputSchema?: Record<string, unknown>;
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

function valid(frame: unknown): frame is Frame {
  if (!frame || typeof frame !== "object") return false;
  const value = frame as Partial<Frame>;
  if (value.version !== VERSION || typeof value.id !== "string" || typeof value.stepId !== "string")
    return false;
  if (value.op === "cancel") return true;
  return (
    value.op === "run" &&
    typeof value.model === "string" &&
    typeof value.prompt === "string" &&
    typeof value.timeoutMs === "number" &&
    Number.isInteger(value.timeoutMs) &&
    value.timeoutMs >= 1
  );
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
    const thread = isolatedCodex().startThread({
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
    for await (const event of events) {
      totalBytes += Buffer.byteLength(JSON.stringify(event));
      if (totalBytes > MAX_STREAM_BYTES) throw new Error("stream_limit");
      if (event.type === "item.completed" && event.item.type === "agent_message") {
        finalText = event.item.text;
      } else if (event.type === "turn.completed") {
        completed = true;
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
    emit({ id: frame.id, stepId: frame.stepId, op: "result", text: finalText });
  } catch (error) {
    emit({
      id: frame.id,
      stepId: frame.stepId,
      op: controller.signal.aborted ? "cancelled" : "failed",
      code: error instanceof Error ? error.message.slice(0, 120) : "sdk_error",
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
