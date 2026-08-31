import { renderSafeMarkdown } from "./markdownRenderer";

const WORKER_THRESHOLD_BYTES = 16 * 1_024;
const CACHE_MAX_ENTRIES = 100;
const CACHE_MAX_BYTES = 8 * 1_024 * 1_024;
const MAX_WORKER_IN_FLIGHT = 2;
const cache = new Map<string, { content: string; html: string; bytes: number }>();
const inFlight = new Map<string, { content: string; promise: Promise<string> }>();
let cacheBytes = 0;
let worker: Worker | null = null;
let nextRequestId = 1;
const pending = new Map<number, { resolve: (html: string) => void; reject: (error: Error) => void }>();
const workerQueue: Array<{ content: string; resolve: (html: string) => void; reject: (error: Error) => void }> = [];
let workerQueueHead = 0;

function contentKey(messageId: string, content: string): string {
  let hash = 2_166_136_261;
  for (let index = 0; index < content.length; index += 1) {
    hash ^= content.charCodeAt(index);
    hash = Math.imul(hash, 16_777_619);
  }
  return `${messageId}:${content.length}:${hash >>> 0}`;
}

function cacheRenderedValue(key: string, content: string, html: string) {
  const encoder = new TextEncoder();
  const bytes = encoder.encode(content).byteLength + encoder.encode(html).byteLength;
  const previous = cache.get(key);
  if (previous) cacheBytes -= previous.bytes;
  cache.set(key, { content, html, bytes });
  cacheBytes += bytes;
  while (cache.size > CACHE_MAX_ENTRIES || cacheBytes > CACHE_MAX_BYTES) {
    const oldest = cache.entries().next().value as [string, { content: string; html: string; bytes: number }] | undefined;
    if (!oldest) break;
    cache.delete(oldest[0]);
    cacheBytes -= oldest[1].bytes;
  }
}

function sharedWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./markdown.worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (event: MessageEvent<{ id: number; html: string }>) => {
    const payload = event.data;
    if (!payload || !Number.isSafeInteger(payload.id) || typeof payload.html !== "string") {
      failWorker(new Error("Markdown worker returned an invalid response"));
      return;
    }
    const request = pending.get(payload.id);
    if (!request) return;
    pending.delete(payload.id);
    request.resolve(payload.html);
    drainWorkerQueue();
  };
  worker.onerror = () => failWorker(new Error("Markdown worker failed"));
  worker.onmessageerror = () => failWorker(new Error("Markdown worker response could not be decoded"));
  return worker;
}

function failWorker(error: Error) {
  worker?.terminate();
  worker = null;
  for (const request of pending.values()) request.reject(error);
  pending.clear();
  for (; workerQueueHead < workerQueue.length; workerQueueHead += 1) {
    workerQueue[workerQueueHead].reject(error);
  }
  workerQueue.length = 0;
  workerQueueHead = 0;
}

function drainWorkerQueue() {
  while (pending.size < MAX_WORKER_IN_FLIGHT && workerQueueHead < workerQueue.length) {
    const request = workerQueue[workerQueueHead++];
    const id = nextRequestId++;
    pending.set(id, request);
    try {
      sharedWorker().postMessage({ id, content: request.content });
    } catch (cause) {
      pending.delete(id);
      request.reject(cause instanceof Error ? cause : new Error("Markdown worker could not accept work"));
    }
  }
  if (workerQueueHead === workerQueue.length) {
    workerQueue.length = 0;
    workerQueueHead = 0;
  }
}

function renderInWorker(content: string): Promise<string> {
  return new Promise((resolve, reject) => {
    workerQueue.push({ content, resolve, reject });
    drainWorkerQueue();
  });
}

function renderWhenIdle(content: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const run = () => {
      try {
        resolve(renderSafeMarkdown(content));
      } catch (cause) {
        reject(cause);
      }
    };
    const idleWindow = window as Window & {
      requestIdleCallback?: (callback: () => void, options: { timeout: number }) => number;
    };
    if (idleWindow.requestIdleCallback) {
      idleWindow.requestIdleCallback(run, { timeout: 50 });
    } else {
      globalThis.setTimeout(run, 0);
    }
  });
}

export async function renderFinalMarkdown(messageId: string, content: string): Promise<string> {
  const key = contentKey(messageId, content);
  const cached = cache.get(key);
  if (cached?.content === content) {
    cache.delete(key);
    cache.set(key, cached);
    return cached.html;
  }
  const existing = inFlight.get(key);
  if (existing?.content === content) return existing.promise;
  const promise = (async () => {
    const bytes = new TextEncoder().encode(content).byteLength;
    let html: string;
    try {
      html = bytes >= WORKER_THRESHOLD_BYTES
        ? await renderInWorker(content)
        : await renderWhenIdle(content);
    } catch {
      html = await renderWhenIdle(content);
    }
    cacheRenderedValue(key, content, html);
    return html;
  })();
  inFlight.set(key, { content, promise });
  try {
    return await promise;
  } finally {
    if (inFlight.get(key)?.promise === promise) inFlight.delete(key);
  }
}
