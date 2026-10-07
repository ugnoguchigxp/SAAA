import { expect, test } from "bun:test";

import { readMediaNdjson, MediaStreamError } from "../src/features/media/mediaHttpStream";

const runId = "00000000-0000-4000-8000-000000000001";

function streamOf(parts: Uint8Array[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const part of parts) controller.enqueue(part);
      controller.close();
    },
  });
}

function textStream(parts: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return streamOf(parts.map((part) => encoder.encode(part)));
}

function progress(seq: number): string {
  return JSON.stringify({
    version: 1,
    runId,
    seq,
    type: "progress",
    progress: { phase: "starting", jobId: null, progress: null },
  });
}

function terminal(seq: number): string {
  return JSON.stringify({
    version: 1,
    runId,
    seq,
    type: "terminal",
    output: {
      runId,
      result: {
        kind: "image",
        model: "lab-image",
        jobId: null,
        artifacts: [
          {
            id: "a",
            contentUrl: "/preview/content",
            metadataUrl: null,
            mimeType: "image/png",
            metadata: {},
          },
        ],
      },
      error: null,
    },
  });
}

test("ndjson accepts a carriage return before the line break", async () => {
  const output = await readMediaNdjson(
    textStream([`${progress(1)}\r\n${terminal(2)}\r\n`]),
    runId,
    () => {},
  );
  expect(output.runId).toBe(runId);
});

test("ndjson accepts a line split across chunks and a gap in seq", async () => {
  const line = `${progress(1)}\n${terminal(3)}\n`;
  const split = Math.floor(line.length / 2);
  const phases: string[] = [];
  const output = await readMediaNdjson(
    textStream([line.slice(0, split), line.slice(split)]),
    runId,
    (value) => phases.push(value.phase),
  );
  expect(phases).toEqual(["starting"]);
  expect(output.result?.kind).toBe("image");
});

test("ndjson rejects a reversed seq, another run, invalid json, and eof before terminal", async () => {
  await expect(
    readMediaNdjson(
      textStream([`${progress(2)}\n${progress(2)}\n${terminal(3)}\n`]),
      runId,
      () => {},
    ),
  ).rejects.toBeInstanceOf(MediaStreamError);
  const other = progress(1).replace(runId, "00000000-0000-4000-8000-000000000002");
  await expect(readMediaNdjson(textStream([`${other}\n`]), runId, () => {})).rejects.toBeInstanceOf(
    MediaStreamError,
  );
  await expect(readMediaNdjson(textStream(["{"]), runId, () => {})).rejects.toBeInstanceOf(
    MediaStreamError,
  );
  await expect(
    readMediaNdjson(textStream([`${progress(1)}\n`]), runId, () => {}),
  ).rejects.toBeInstanceOf(MediaStreamError);
});

test("ndjson rejects progress after the terminal and a line over 1MiB", async () => {
  await expect(
    readMediaNdjson(textStream([`${terminal(1)}\n${progress(2)}\n`]), runId, () => {}),
  ).rejects.toBeInstanceOf(MediaStreamError);
  const huge = `{"version":1,"runId":"${runId}","seq":1,"type":"progress","progress":{"phase":"${"x".repeat(1024 * 1024)}","jobId":null,"progress":null}}\n`;
  await expect(readMediaNdjson(textStream([huge]), runId, () => {})).rejects.toBeInstanceOf(
    MediaStreamError,
  );
});

test("ndjson decodes a character split between chunks", async () => {
  const prefix = `{"version":1,"runId":"${runId}","seq":1,"type":"progress","progress":{"phase":"`;
  const suffix = `","jobId":null,"progress":null}}\n${terminal(2)}\n`;
  const character = new TextEncoder().encode("あ");
  const output = await readMediaNdjson(
    streamOf([
      new TextEncoder().encode(prefix),
      character.slice(0, 1),
      character.slice(1),
      new TextEncoder().encode(suffix),
    ]),
    runId,
    () => {},
  );
  expect(output.runId).toBe(runId);
});
