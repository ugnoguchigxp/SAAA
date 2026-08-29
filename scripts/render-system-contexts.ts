import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { createAppCatalog } from "../.s11tnext/catalog.generated.ts";

const projectRoot = fileURLToPath(new URL("../", import.meta.url));
const artifactPath = fileURLToPath(new URL("../.s11tnext/catalog.json", import.meta.url));
const outputPath = fileURLToPath(new URL("../.s11tnext/codex-read-only.txt", import.meta.url));
const artifact: unknown = JSON.parse(await readFile(artifactPath, "utf8"));
const catalog = createAppCatalog(artifact);
const invocation = catalog.bind({
  instructionLocale: "en-US",
  trailingNewline: false,
})("codex.read-only", {});

if (invocation.role !== "system") {
  throw new Error(`codex.read-only must render as a system message, received ${invocation.role}`);
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
