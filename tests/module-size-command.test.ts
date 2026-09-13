import { describe, expect, test } from "bun:test";
import { runSizeCommand } from "../scripts/module-size-baseline";
import { join } from "node:path";
import { tmpdir } from "node:os";

describe("module-size command", () => {
  test("rejects unknown commands and missing baselines", () => {
    expect(() => runSizeCommand("rewrite", "missing.json")).toThrow("usage:");
    expect(() => runSizeCommand("check", join(tmpdir(), "missing-baseline.json"))).toThrow(
      "missing baseline",
    );
  });
});
