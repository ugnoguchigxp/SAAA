import { expect, test } from "bun:test";
import { newWarnings, warningCounts } from "../scripts/clippy-ratchet";

test("Clippy ratchet rejects a new warning and an extra occurrence of an existing warning", () => {
  const diagnostic = (file: string, message: string) =>
    JSON.stringify({
      reason: "compiler-message",
      package_id: "path+file:///project/src-tauri#saaa@0.1.0",
      message: {
        level: "warning",
        code: { code: "unused_imports" },
        message,
        spans: [{ file_name: file, is_primary: true }],
      },
    });
  const actual = warningCounts([
    diagnostic("src/a.rs", "unused import: `A`"),
    diagnostic("src/a.rs", "unused import: `A`"),
    diagnostic("src/b.rs", "unused import: `B`"),
  ].join("\n"));
  expect(newWarnings(actual, {
    "src/a.rs | unused_imports | unused import: `A`": 1,
  })).toEqual([
    "src/a.rs | unused_imports | unused import: `A` (2, baseline 1)",
    "src/b.rs | unused_imports | unused import: `B` (1, baseline 0)",
  ]);
});
