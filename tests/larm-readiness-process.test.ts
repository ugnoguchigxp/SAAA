import { describe, expect, test } from "bun:test";
import {
  applicationDetectedForbiddenData,
  capturedApplicationStream,
  consumeBounded,
  runBoundedChild,
} from "../scripts/larm-readiness/process";
import { ForbiddenDataScanner, RunnerError } from "../scripts/larm-readiness.ts";

function streamFrom(chunks: Uint8Array[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
}

describe("LARM readiness process helpers", () => {
  test("consumes a bounded stream and rejects overflow or forbidden bytes", async () => {
    const scanner = new ForbiddenDataScanner(["SECRET"]);
    expect(await consumeBounded(streamFrom([Buffer.from("ok")]), 16, scanner, () => undefined, () => undefined)).toBe(2);
    await expect(consumeBounded(
      streamFrom([Buffer.from("this is too long")]),
      4,
      new ForbiddenDataScanner([]),
      () => undefined,
      () => undefined,
    )).rejects.toThrow(RunnerError);
    await expect(consumeBounded(
      streamFrom([Buffer.from("has SECRET inside")]),
      64,
      new ForbiddenDataScanner(["SECRET"]),
      () => undefined,
      () => undefined,
    )).rejects.toThrow(RunnerError);
  });

  test("runs a bounded child to completion", async () => {
    const scanner = new ForbiddenDataScanner([]);
    const result = await runBoundedChild({
      command: ["/bin/echo", "ready"],
      environment: { PATH: process.env.PATH ?? "/usr/bin:/bin" },
      limit: 4_096,
      deadlineMs: 2_000,
      scanner,
    });
    expect(result.exitCode).toBe(0);
  });

  test("flags forbidden application streams without treating clean output as a leak", async () => {
    const leaked = new ForbiddenDataScanner(["SECRET"]);
    leaked.scan(Buffer.from("SECRET"));
    expect(applicationDetectedForbiddenData({
      stdoutScanner: leaked,
      stderrScanner: new ForbiddenDataScanner([]),
    } as never)).toBe(true);
    expect(applicationDetectedForbiddenData({
      stdoutScanner: new ForbiddenDataScanner([]),
      stderrScanner: new ForbiddenDataScanner([]),
    } as never)).toBe(false);
    const child = { pid: 1, exitCode: 0, kill() { return true; } } as never;
    const capture: Buffer[] = [];
    expect(await capturedApplicationStream(streamFrom([Buffer.from("ok")]), new ForbiddenDataScanner([]), child, capture))
      .toEqual({ ok: true });
    expect(Buffer.concat(capture).toString()).toBe("ok");
    expect(await capturedApplicationStream(
      streamFrom([Buffer.from("has SECRET inside")]),
      new ForbiddenDataScanner(["SECRET"]),
      child,
      [],
    )).toEqual({ ok: false });
  });
});
