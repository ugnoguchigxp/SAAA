import { describe, expect, test } from "bun:test";
import { evaluate, type BaselineFile, type SizeRecord } from "../scripts/module-size";
import { supplementBaseline } from "../scripts/module-size-baseline";

const record = (path: string, total: number, production = total): SizeRecord => ({
  path,
  total,
  production,
});
const baseline: BaselineFile = {
  generatedAt: "original",
  files: { "src/old.ts": { total: 101, production: 101 } },
};

describe("module-size registration", () => {
  test("rejects unregistered files even below the hard budget", () => {
    expect(evaluate([record("src/new.ts", 10)], { generatedAt: "test", files: {} })).toEqual([
      "src/new.ts: missing baseline; run bun run size:register",
    ]);
  });
  test("supplements without resetting old values, timestamp or mutating the input", () => {
    const next = supplementBaseline(
      [record("src/old.ts", 110), record("src/new.ts", 20)],
      baseline,
    );
    expect(next.generatedAt).toBe("original");
    expect(next.files["src/old.ts"]).toEqual({ total: 101, production: 101 });
    expect(next.files["src/new.ts"]).toEqual({ total: 20, production: 20 });
    expect(Object.keys(baseline.files)).toEqual(["src/old.ts"]);
    expect(supplementBaseline([record("src/old.ts", 110), record("src/new.ts", 20)], next)).toEqual(
      next,
    );
  });
  test("accepts ceil(101 * 1.1), rejects the next line and refuses to reset it", () => {
    expect(evaluate([record("src/old.ts", 112)], baseline)).toEqual([]);
    expect(evaluate([record("src/old.ts", 113)], baseline)).toContain(
      "src/old.ts: 113 exceeds ratchet 112 (baseline 101)",
    );
    expect(() => supplementBaseline([record("src/old.ts", 113)], baseline)).toThrow(
      "exceeds ratchet",
    );
  });
  test("refuses to grandfather oversized new Rust and TypeScript files", () => {
    expect(() => supplementBaseline([record("src/new.ts", 701)], baseline)).toThrow(
      "hard budget 700",
    );
    expect(() =>
      supplementBaseline([record("src-tauri/src/new.rs", 1800, 1601)], baseline),
    ).toThrow("hard budget 1600");
  });
  test("uses production lines for the Rust ratchet", () => {
    const rust = {
      generatedAt: "test",
      files: { "src-tauri/src/new.rs": { total: 1000, production: 100 } },
    };
    expect(evaluate([record("src-tauri/src/new.rs", 2000, Math.ceil(100 * 1.1))], rust)).toEqual(
      [],
    );
    expect(
      evaluate([record("src-tauri/src/new.rs", 2000, Math.ceil(100 * 1.1) + 1)], rust)[0],
    ).toContain("exceeds ratchet");
  });
  test("leaves deleted and moved entries for explicit review", () => {
    const next = supplementBaseline([record("src/moved.ts", 80)], baseline);
    expect(next.files["src/old.ts"]).toEqual(baseline.files["src/old.ts"]);
    expect(evaluate([record("src/moved.ts", 80)], next)).toEqual([
      "src/old.ts: stale baseline; review deletion or transfer its baseline when moving a file",
    ]);
    const moved = { ...baseline, files: { "src/moved.ts": baseline.files["src/old.ts"] } };
    expect(evaluate([record("src/moved.ts", 112)], moved)).toEqual([]);
    expect(evaluate([record("src/moved.ts", 113)], moved)[0]).toContain("exceeds ratchet");
  });
});
