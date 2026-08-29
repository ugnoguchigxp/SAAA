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
  expect(invocation.content.text).toContain(
    "SAAA transcribes voice input before invoking you and passes the finalized transcript as the user message text.",
  );
  expect(invocation.content.text).toContain(
    "do not claim that speech recognition is unavailable or required.",
  );
});

test("renders voice transcription context for every conversation provider", () => {
  const invocation = createAppCatalog(artifact).bind({
    instructionLocale: "en-US",
    trailingNewline: false,
  })("conversation.respond", {});

  expect(invocation.role).toBe("system");
  expect(invocation.content.text).toBe(
    projectFile(".s11tnext/conversation-respond.txt"),
  );
  expect(invocation.content.text).toContain(
    "SAAA transcribes voice input before invoking you and passes the finalized transcript as the user message text.",
  );
  expect(invocation.content.text).toContain(
    "do not claim that speech recognition is unavailable or required.",
  );
});

test("keeps the system context outside Rust program code", () => {
  const rustSource = [
    projectFile("src-tauri/src/lib.rs"),
    projectFile("src-tauri/src/runtime/codex_process.rs"),
    projectFile("src-tauri/src/runtime/turns.rs"),
  ].join("\n");

  expect(rustSource).toContain('include_str!("../../.s11tnext/codex-read-only.txt")');
  expect(rustSource).toContain(
    'include_str!("../../.s11tnext/conversation-respond.txt")',
  );
  expect(rustSource).toContain('"developerInstructions": CODEX_READ_ONLY_SYSTEM_CONTEXT');
  expect(rustSource).toContain("content: CONVERSATION_SYSTEM_CONTEXT.to_string()");
  expect(rustSource).not.toContain("Operate read-only. Do not modify files");
  expect(rustSource).not.toContain("SAAA transcribes voice input before invoking you");
});
