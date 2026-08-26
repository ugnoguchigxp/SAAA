import { describe, expect, test } from "bun:test";
import { meetingPreflight } from "../src/lib/runtime";

describe("meeting contracts", () => {
  test("exposes the preflight command through the typed runtime boundary", () => {
    expect(typeof meetingPreflight).toBe("function");
  });
});
