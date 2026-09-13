import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verifyMacBundle } from "./macos-bundle-smoke";
import { runDesktopSmoke } from "./desktop-smoke-process";

const args = process.argv.slice(2);
if (args.length !== 0 && (args.length !== 2 || args[0] !== "--report-dir" || !args[1])) {
  console.error("usage: bun run desktop:smoke [--report-dir DIRECTORY]");
  process.exit(64);
}
const root = fileURLToPath(new URL("..", import.meta.url));
const reportDir = args[1] ? resolve(args[1]) : mkdtempSync(join(tmpdir(), "saaa-smoke-report-"));
const mac = process.platform === "darwin";
try {
  await runDesktopSmoke({
    root, reportDir,
    build: mac ? ["bunx", "tauri", "build", "--debug", "--bundles", "app"]
      : ["bunx", "tauri", "build", "--debug", "--no-bundle"],
    executable: [mac ? join(root, "src-tauri/target/debug/bundle/macos/SAAA.app/Contents/MacOS/saaa")
      : join(root, `src-tauri/target/debug/saaa${process.platform === "win32" ? ".exe" : ""}`)],
    verifyBundle: mac ? () => verifyMacBundle(root) : undefined,
  });
  console.log("Desktop smoke passed: packaged frontend reported IPC ready.");
} catch (cause) {
  console.error(cause instanceof Error ? cause.message : String(cause));
  process.exitCode = 1;
} finally {
  console.log(`Desktop smoke report: ${reportDir}`);
}
