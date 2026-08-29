import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { createAppCatalog } from "../.s11tnext/catalog.generated.ts";

const projectRoot = fileURLToPath(new URL("../", import.meta.url));
const artifactPath = fileURLToPath(new URL("../.s11tnext/catalog.json", import.meta.url));
const artifact: unknown = JSON.parse(await readFile(artifactPath, "utf8"));
const catalog = createAppCatalog(artifact);
const contexts = [
  { key: "codex.read-only", output: ".s11tnext/codex-read-only.txt" },
  { key: "conversation.respond", output: ".s11tnext/conversation-respond.txt" },
] as const;

for (const context of contexts) {
  const outputPath = fileURLToPath(new URL(`../${context.output}`, import.meta.url));
  const invocation = catalog.bind({
    instructionLocale: "en-US",
    trailingNewline: false,
  })(context.key, {});

  if (invocation.role !== "system") {
    throw new Error(
      `${context.key} must render as a system message, received ${invocation.role}`,
    );
  }

  if (process.argv.includes("--check")) {
    const current = await readFile(outputPath, "utf8").catch(() => undefined);
    if (current !== invocation.content.text) {
      console.error(
        `${outputPath.slice(projectRoot.length)} is stale; run bun run s11tnext:build`,
      );
      process.exitCode = 1;
    }
  } else {
    await writeFile(outputPath, invocation.content.text, "utf8");
  }
}
