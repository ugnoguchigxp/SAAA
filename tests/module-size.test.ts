import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { evaluate, productionLines, walk, type BaselineFile } from "../scripts/module-size";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("module-size ratchet", () => {
  test("counts Rust production before a terminal test module", () => {
    const source = "fn production() {}\n\n#[cfg(test)]\nmod tests {\n  #[test]\n  fn works() {}\n}\n";
    expect(productionLines(source, "src/example.rs")).toBe(2);
    expect(productionLines(source, "src/example.ts")).toBe(8);
    const platformTest = "fn production() {}\n\n#[cfg(all(test, target_os = \"macos\"))]\nmod tests {}\n";
    expect(productionLines(platformTest, "src/example.rs")).toBe(2);
  });

  test("does not hide production code after an earlier test module", () => {
    const source = [
      "fn first() {}",
      "",
      "#[cfg(test)]",
      "mod tests { fn first_works() {} }",
      "",
      "fn second() {}",
      "",
      "#[cfg(all(test, target_os = \"macos\"))]",
      "mod tests { fn second_works() {} }",
      "",
    ].join("\n");
    expect(productionLines(source, "src/example.rs")).toBe(7);

    const nonTerminal = "fn first() {}\n\n#[cfg(test)]\nmod tests { fn works() {} }\n\nfn second() {}\n";
    expect(productionLines(nonTerminal, "src/example.rs")).toBe(7);
  });

  test("rejects new Rust modules above the hard production budget", () => {
    const baseline: BaselineFile = { generatedAt: "test", files: {} };
    expect(evaluate([
      { path: "src-tauri/src/oversized.rs", total: 1_601, production: 1_601 },
    ], baseline)).toEqual([
      "src-tauri/src/oversized.rs: 1601 exceeds hard budget 1600",
    ]);
  });

  test("rejects oversized new TypeScript modules outside scripts", () => {
    const baseline: BaselineFile = { generatedAt: "test", files: {} };
    expect(evaluate([
      { path: "src/lib/oversized.ts", total: 701, production: 701 },
    ], baseline)).toEqual([
      "src/lib/oversized.ts: 701 exceeds hard budget 700",
    ]);
  });

  test("applies the App.tsx hard budget of 450", () => {
    const baseline: BaselineFile = { generatedAt: "test", files: {} };
    expect(evaluate([
      { path: "src/App.tsx", total: 451, production: 451 },
    ], baseline)).toEqual([
      "src/App.tsx: 451 exceeds hard budget 450",
    ]);
  });

  test("does not follow symlinks or include special directory entries", () => {
    const directory = mkdtempSync(join(tmpdir(), "saaa-module-size-"));
    temporaryDirectories.push(directory);
    const sourceDirectory = join(directory, "source");
    mkdirSync(sourceDirectory);
    const source = join(sourceDirectory, "kept.ts");
    writeFileSync(source, "export const kept = true;\n");
    symlinkSync(sourceDirectory, join(directory, "directory-link"));
    symlinkSync(source, join(directory, "file-link.ts"));

    expect(walk(directory)).toEqual([source]);
  });
});
