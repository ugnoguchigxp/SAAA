import { installJsdom } from "./jsdomGlobals";
import { expect, test } from "bun:test";
import fixtures from "./fixtures/ipc-receivers.json";
import {
  appSnapshotSchema,
  runtimeEventSchema,
  meetingEventSchema,
  voiceAsrEventSchema,
  guardedReceiver,
  parseIpc,
} from "../src/lib/ipcValidation";

test("Rust-serialized receiver fixtures satisfy frontend schemas", () => {
  expect(appSnapshotSchema.safeParse(fixtures.snapshot).success).toBe(true);
  expect(runtimeEventSchema.safeParse(fixtures.runtime).success).toBe(true);
  expect(meetingEventSchema.safeParse(fixtures.meeting).success).toBe(true);
  expect(voiceAsrEventSchema.safeParse(fixtures.asr).success).toBe(true);
});
test("malformed payloads fail without echoing private data", () => {
  for (const value of [
    null,
    {},
    { ...fixtures.runtime, type: "unknown", secret: "private-fixture" },
    { ...fixtures.runtime, runId: 12 },
  ]) {
    try {
      parseIpc(runtimeEventSchema, value, "runtime");
      throw new Error("accepted");
    } catch (error) {
      expect(String(error)).toContain("Invalid runtime IPC");
      expect(String(error)).not.toContain("private-fixture");
    }
  }
  expect(voiceAsrEventSchema.safeParse({ ...fixtures.asr, revision: Infinity }).success).toBe(
    false,
  );
  expect(voiceAsrEventSchema.safeParse({ ...fixtures.asr, revision: -1 }).success).toBe(false);
  expect(voiceAsrEventSchema.safeParse({ ...fixtures.asr, startMs: 1001 }).success).toBe(false);
  expect(
    appSnapshotSchema.safeParse({
      ...fixtures.snapshot,
      conversations: new Array(10001).fill(fixtures.snapshot.conversations[0]),
    }).success,
  ).toBe(false);
  expect(meetingEventSchema.safeParse({ ...fixtures.meeting, sequence: NaN }).success).toBe(false);
});
test("invalid or foreign events quarantine the receiver without a synthetic terminal", () => {
  const env = installJsdom();
  try {
    const received: unknown[] = [];
    let interrupted = 0;
    const receiver = guardedReceiver(
      runtimeEventSchema,
      "runtime",
      (event) => received.push(event),
      (event) => event.runId === "run-fixture",
      () => {
        interrupted += 1;
      },
    );
    receiver(fixtures.runtime);
    receiver({ ...fixtures.runtime, runId: "foreign-run" });
    receiver({ type: "cancelled", runId: "run-fixture" });
    receiver({ type: "unknown" });
    expect(received).toEqual([fixtures.runtime]);
    expect(interrupted).toBe(1);
  } finally {
    env.dom.window.close();
    env.restore();
  }
});
