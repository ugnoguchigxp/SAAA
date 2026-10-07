import { z } from "zod";

import {
  outputSchema,
  progressSchema,
  type MediaOutput,
  type MediaProgress,
} from "./mediaContracts";

const LINE_LIMIT = 1024 * 1024;

const envelopeSchema = z.discriminatedUnion("type", [
  z.object({
    version: z.literal(1),
    runId: z.string().uuid(),
    seq: z.number().int().positive(),
    type: z.literal("progress"),
    progress: progressSchema,
  }),
  z.object({
    version: z.literal(1),
    runId: z.string().uuid(),
    seq: z.number().int().positive(),
    type: z.literal("terminal"),
    output: outputSchema,
  }),
  z.object({
    version: z.literal(1),
    runId: z.string().uuid(),
    seq: z.number().int().positive(),
    type: z.literal("error"),
    error: z.object({ code: z.string(), message: z.string() }),
  }),
]);

export class MediaStreamError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MediaStreamError";
  }
}

/** Reads one NDJSON media stream. Gaps in seq are allowed. This does not retry the request. */
export async function readMediaNdjson(
  body: ReadableStream<Uint8Array>,
  runId: string,
  onProgress: (value: MediaProgress) => void,
): Promise<MediaOutput> {
  const reader = body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let pending = "";
  let lastSeq = 0;
  let terminal: MediaOutput | null = null;

  const takeLines = (final: boolean) => {
    while (true) {
      const newline = pending.indexOf("\n");
      if (newline < 0) {
        if (utf8Size(pending) > LINE_LIMIT) {
          throw new MediaStreamError("応答が大きすぎます。");
        }
        break;
      }
      const line = stripCarriageReturn(pending.slice(0, newline));
      pending = pending.slice(newline + 1);
      acceptLine(line);
    }
    if (final && stripCarriageReturn(pending).length > 0) {
      throw new MediaStreamError("生成結果を受信する前に通信が終わりました。");
    }
  };

  const acceptLine = (line: string) => {
    if (line.length === 0) return;
    if (utf8Size(line) > LINE_LIMIT) throw new MediaStreamError("応答が大きすぎます。");
    if (terminal) throw new MediaStreamError("終端の後に応答が続きました。");
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      throw new MediaStreamError("生成応答を読み取れませんでした。");
    }
    const envelope = envelopeSchema.safeParse(parsed);
    if (!envelope.success) throw new MediaStreamError("生成応答を読み取れませんでした。");
    if (envelope.data.runId !== runId) {
      throw new MediaStreamError("生成結果の識別子が一致しません。");
    }
    if (envelope.data.seq <= lastSeq) {
      throw new MediaStreamError("生成応答の順序が不正です。");
    }
    lastSeq = envelope.data.seq;
    if (envelope.data.type === "progress") {
      onProgress(envelope.data.progress);
      return;
    }
    if (envelope.data.type === "error") {
      throw new MediaStreamError(envelope.data.error.message);
    }
    if (envelope.data.output.runId !== runId) {
      throw new MediaStreamError("生成結果の識別子が一致しません。");
    }
    terminal = envelope.data.output;
  };

  try {
    while (true) {
      const next = await reader.read();
      if (next.done) {
        takeLines(false);
        const rest = decoder.decode();
        if (rest) pending += rest;
        takeLines(true);
        break;
      }
      pending += decoder.decode(next.value, { stream: true });
      takeLines(false);
    }
  } catch (error) {
    if (error instanceof MediaStreamError) throw error;
    throw new MediaStreamError("生成応答を読み取れませんでした。");
  } finally {
    try {
      reader.releaseLock();
    } catch {
      // The reader is already released when the stream closed.
    }
  }
  if (!terminal) throw new MediaStreamError("生成結果を受信する前に通信が終わりました。");
  return terminal;
}

function stripCarriageReturn(line: string): string {
  return line.endsWith("\r") ? line.slice(0, -1) : line;
}

function utf8Size(text: string): number {
  return new TextEncoder().encode(text).byteLength;
}
