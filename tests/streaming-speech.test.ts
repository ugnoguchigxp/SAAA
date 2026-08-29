import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { StreamingSpeechChunker } from "../src/lib/streamingSpeech";

describe("streaming speech chunks", () => {
  test("starts at a natural sentence end before the model response completes", () => {
    const chunker = new StreamingSpeechChunker();
    expect(chunker.push("これはまだ短い")).toEqual([]);
    expect(chunker.push("ですが、ここで最初の文が終わります。続き")).toEqual([
      "これはまだ短いですが、ここで最初の文が終わります。",
    ]);
    expect(chunker.finish()).toEqual(["続き"]);
  });

  test("uses a pause near the target size when no sentence end arrives", () => {
    const chunker = new StreamingSpeechChunker({ minSentenceChars: 8, targetChars: 20, maxChars: 30 });
    expect(chunker.push("文末がなかなか来ないので、途中の読点を使って読み上げを始めます")).toEqual([
      "文末がなかなか来ないので、",
    ]);
  });

  test("enforces a hard limit for an unbroken stream", () => {
    const chunker = new StreamingSpeechChunker({ minSentenceChars: 4, targetChars: 6, maxChars: 8 });
    expect(chunker.push("あいうえおかきくけこ")).toEqual(["あいうえおかきく"]);
    expect(chunker.finish()).toEqual(["けこ"]);
  });

  test("flushes a short final fragment exactly once", () => {
    const chunker = new StreamingSpeechChunker();
    expect(chunker.push("短い回答")).toEqual([]);
    expect(chunker.finish()).toEqual(["短い回答"]);
    expect(chunker.finish()).toEqual([]);
  });

  test("does not treat a URL query marker as a sentence end", () => {
    const chunker = new StreamingSpeechChunker({ minSentenceChars: 8, targetChars: 80, maxChars: 100 });
    expect(chunker.push("詳細は https://example.com/search?q=test を確認してください。次です")).toEqual([
      "詳細は https://example.com/search?q=test を確認してください。",
    ]);
  });

  test("feeds deltas into speech and only flushes the tail at completion", () => {
    const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
    const runtimeHandler = app.slice(
      app.indexOf("function handleRuntimeEvent"),
      app.indexOf("async function stopActiveRun"),
    );
    expect(runtimeHandler).toContain("appendStreamingSpeech(event.runId, event.text)");
    expect(runtimeHandler).toContain("finishStreamingSpeech(event.runId, event.message.content)");
  });
});
