import { Codex } from "@openai/codex-sdk";

// This is intentionally an import/constructor probe only. It never starts a
// Codex thread, reads auth material, or lets an agent touch a workspace.
const runtime = typeof Bun === "undefined" ? "node" : `bun-${Bun.version}`;
const codex = new Codex();

if (!codex) {
  throw new Error("Codex SDK constructor did not return an instance");
}

console.log(
  JSON.stringify({
    sdk: "@openai/codex-sdk",
    runtime,
    import: "ok",
    constructor: "ok",
    threadExecution: "not-run",
  }),
);
