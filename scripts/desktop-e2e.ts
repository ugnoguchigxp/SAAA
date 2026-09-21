import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runDesktopSmoke } from "./desktop-smoke-process";
import { verifyMacBundle } from "./macos-bundle-smoke";

export const DESKTOP_E2E_CHECKS = [
  "frontendRendered",
  "ipcReady",
  "snapshotLoaded",
  "primaryConversationLoaded",
  "databaseInitialized",
  "situationSampled",
] as const;

export async function main(args = process.argv.slice(2)): Promise<number> {
  if (args.length !== 0 && (args.length !== 2 || args[0] !== "--report-dir" || !args[1])) {
    console.error("usage: bun run desktop:e2e [--report-dir DIRECTORY]");
    return 64;
  }
  const root = fileURLToPath(new URL("..", import.meta.url));
  const reportDir = args[1] ? resolve(args[1]) : mkdtempSync(join(tmpdir(), "saaa-e2e-report-"));
  const mac = process.platform === "darwin";
  const requiredChecks = [
    ...DESKTOP_E2E_CHECKS,
    ...(mac ? (["speakerRuntimeAvailable"] as const) : []),
  ];
  try {
    await runDesktopSmoke({
      root,
      reportDir,
      build: mac
        ? ["bunx", "tauri", "build", "--debug", "--bundles", "app"]
        : ["bunx", "tauri", "build", "--debug", "--no-bundle"],
      executable: [
        mac
          ? join(root, "src-tauri/target/debug/bundle/macos/SAAA.app/Contents/MacOS/saaa")
          : join(root, `src-tauri/target/debug/saaa${process.platform === "win32" ? ".exe" : ""}`),
      ],
      verifyBundle: mac ? () => verifyMacBundle(root) : undefined,
      requiredChecks: [...requiredChecks],
    });
    console.log(`Desktop E2E passed (${requiredChecks.join(", ")}).`);
    return 0;
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause));
    return 1;
  } finally {
    console.log(`Desktop E2E report: ${reportDir}`);
  }
}

if (import.meta.main) process.exitCode = await main();
