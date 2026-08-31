/// <reference lib="webworker" />

import { renderSafeMarkdown } from "./markdownRenderer";

self.onmessage = (event: MessageEvent<{ id: number; content: string }>) => {
  const { id, content } = event.data;
  self.postMessage({ id, html: renderSafeMarkdown(content) });
};

export {};
