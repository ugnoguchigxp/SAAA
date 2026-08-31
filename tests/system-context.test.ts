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
  expect(invocation.content.text).toContain("SAAA transcribes voice input before invoking you and passes the finalized transcript as the user message text.");
  expect(invocation.content.text).toContain("do not claim that speech recognition is unavailable or required.");
});

test("renders voice transcription context for every conversation provider", () => {
  const invocation = createAppCatalog(artifact).bind({
    instructionLocale: "en-US",
    trailingNewline: false,
  })("conversation.respond", {});

  expect(invocation.role).toBe("system");
  expect(invocation.content.text).toBe(projectFile(".s11tnext/conversation-respond.txt"));
  expect(invocation.content.text).toContain("SAAA transcribes voice input before invoking you and passes the finalized transcript as the user message text.");
  expect(invocation.content.text).toContain("do not claim that speech recognition is unavailable or required.");
  expect(invocation.content.text).toContain("Use `web_search` when the user's request depends on current or time-sensitive public information");
  expect(invocation.content.text).toContain("Use `fetch_content` when a search result or public URL needs closer reading.");
  expect(invocation.content.text).toContain("the location for a weather request");
  expect(invocation.content.text).toContain("The configured agent name is {{agentNameJson}}.");
  expect(invocation.content.text).toContain("use that exact name whenever you identify or refer to yourself.");
  expect(invocation.content.text).toContain("The configured user name is {{userNameJson}}.");
  expect(invocation.content.text).toContain("The configured regional preferences are {{regionalPreferencesJson}}.");
  expect(invocation.content.text).toContain("Use the configured time zone when interpreting relative dates and times.");
  expect(invocation.content.text).toContain("Use the configured units and currency when the user has not specified alternatives.");
  expect(invocation.content.text).toContain("do not infer, invent, or recall a user name");
  expect(invocation.content.text).toContain("Do not use Markdown headings or headline-style lines.");
});

test("keeps the system context outside Rust program code", () => {
  const rustSource = [
    projectFile("src-tauri/src/lib.rs"),
    projectFile("src-tauri/src/runtime/codex_process.rs"),
    projectFile("src-tauri/src/runtime/conversation_context.rs"),
    projectFile("src-tauri/src/runtime/turns.rs"),
  ].join("\n");

  expect(rustSource).toContain('include_str!("../../.s11tnext/codex-read-only.txt")');
  expect(rustSource).toContain('include_str!("../../../.s11tnext/conversation-respond.txt")');
  expect(rustSource).toContain('"developerInstructions": CODEX_READ_ONLY_SYSTEM_CONTEXT');
  expect(rustSource).toContain("render_conversation_system_context(");
  expect(rustSource).toContain("regional_preferences::load(&connection)");
  expect(rustSource).not.toContain("Operate read-only. Do not modify files");
  expect(rustSource).not.toContain("SAAA transcribes voice input before invoking you");
});
