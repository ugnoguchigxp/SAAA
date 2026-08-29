import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import artifact from "../.s11tnext/catalog.json";
import { createAppCatalog } from "../.s11tnext/catalog.generated";

const projectFile = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

test("renders the Codex system context from the S11tnext catalog", () => {
  const invocation = createAppCatalog(artifact).bind({
    instructionLocale: "en-US",
    trailingNewline: false,
  })("codex.read-only", {});

  expect(invocation.role).toBe("system");
  expect(invocation.content.text).toBe(projectFile(".s11tnext/codex-read-only.txt"));
});

test("keeps the system context outside Rust program code", () => {
  const rustSource = projectFile("src-tauri/src/lib.rs");

  expect(rustSource).toContain('include_str!("../../.s11tnext/codex-read-only.txt")');
  expect(rustSource).toContain('"developerInstructions": CODEX_READ_ONLY_SYSTEM_CONTEXT');
  expect(rustSource).not.toContain("Operate read-only. Do not modify files");
});
