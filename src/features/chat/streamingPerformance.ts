type RunPerformance = {
  runId: string;
  socketReceiveAt: number | null;
  firstDeltaAt: number | null;
  firstPlainPaintAt: number | null;
  responseCompletedAt: number | null;
  markdownPaintAt: number | null;
  streamingCommits: number;
  longTaskCount: number;
  terminal: "completed" | "cancelled" | "failed" | null;
};

export type RunPerformanceSnapshot = Readonly<RunPerformance>;

const MAX_RUNS = 64;
const runs = new Map<string, RunPerformance>();
const messageRuns = new Map<string, string>();

function now(): number {
  return typeof performance === "undefined" ? Date.now() : performance.now();
}

function mark(runId: string, stage: string): void {
  if (typeof performance === "undefined" || typeof performance.mark !== "function") return;
  performance.mark(`saaa:${stage}:${runId}`);
}

function record(runId: string): RunPerformance | null {
  return runs.get(runId) ?? null;
}

export function beginRunPerformance(runId: string): void {
  runs.delete(runId);
  runs.set(runId, {
    runId,
    socketReceiveAt: null,
    firstDeltaAt: null,
    firstPlainPaintAt: null,
    responseCompletedAt: null,
    markdownPaintAt: null,
    streamingCommits: 0,
    longTaskCount: 0,
    terminal: null,
  });
  while (runs.size > MAX_RUNS) runs.delete(runs.keys().next().value as string);
  mark(runId, "run-start");
}

export function recordSocketReceive(runId: string): void {
  const value = record(runId);
  if (!value || value.socketReceiveAt !== null) return;
  value.socketReceiveAt = now();
  mark(runId, "socket-receive");
}

export function recordFirstDelta(runId: string): void {
  const value = record(runId);
  if (!value || value.firstDeltaAt !== null) return;
  value.firstDeltaAt = now();
  mark(runId, "first-delta");
}

export function recordPlainCommit(runId: string): boolean {
  const value = record(runId);
  if (!value) return false;
  value.streamingCommits += 1;
  return value.firstPlainPaintAt === null;
}

export function recordFirstPlainPaint(runId: string): void {
  const value = record(runId);
  if (!value || value.firstPlainPaintAt !== null) return;
  value.firstPlainPaintAt = now();
  mark(runId, "first-plain-paint");
}

export function recordResponseCompleted(runId: string, messageId: string): void {
  const value = record(runId);
  if (!value) return;
  value.responseCompletedAt = now();
  value.terminal = "completed";
  messageRuns.set(messageId, runId);
  mark(runId, "response-completed");
}

export function recordMarkdownPaint(messageId: string): void {
  const runId = messageRuns.get(messageId);
  if (!runId) return;
  const value = record(runId);
  if (!value || value.markdownPaintAt !== null) return;
  value.markdownPaintAt = now();
  messageRuns.delete(messageId);
  mark(runId, "markdown-paint");
}

export function recordRunWithoutMarkdown(runId: string, terminal: "cancelled" | "failed"): void {
  const value = record(runId);
  if (!value) return;
  value.terminal = terminal;
  mark(runId, terminal);
}

export function runPerformanceSnapshot(runId: string): RunPerformanceSnapshot | null {
  const value = record(runId);
  return value ? { ...value } : null;
}

if (typeof PerformanceObserver !== "undefined") {
  try {
    const observer = new PerformanceObserver((entries) => {
      const longTasks = entries.getEntries().filter((entry) => entry.duration >= 50).length;
      if (longTasks === 0) return;
      for (const value of runs.values()) {
        if (value.terminal === null) value.longTaskCount += longTasks;
      }
    });
    observer.observe({ entryTypes: ["longtask"] });
  } catch {
    // Long-task entries are optional. User-timing marks remain available.
  }
}
