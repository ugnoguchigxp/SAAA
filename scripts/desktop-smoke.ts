import { existsSync, mkdtempSync, rmSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const buildArguments = process.platform === "darwin"
  ? ["bunx", "tauri", "build", "--debug", "--bundles", "app"]
  : ["bunx", "tauri", "build", "--debug", "--no-bundle"];
const build = Bun.spawn(buildArguments, { cwd: root, stdout: "inherit", stderr: "inherit" });
if (await build.exited !== 0) process.exit(1);

const executable = process.platform === "darwin"
  ? join(root, "src-tauri/target/debug/bundle/macos/SAAA.app/Contents/MacOS/saaa")
  : join(root, `src-tauri/target/debug/saaa${process.platform === "win32" ? ".exe" : ""}`);
const markerId = `smoke-${Date.now()}`;
const marker = join(tmpdir(), `saaa-frontend-${markerId}.ready`);
const dataDirectory = mkdtempSync(join(tmpdir(), "saaa-desktop-smoke-"));
if (existsSync(marker)) unlinkSync(marker);
const application = Bun.spawn([executable], {
  cwd: root,
  env: {
    ...process.env,
    SAAA_SMOKE_MARKER_ID: markerId,
    SAAA_SMOKE_DATA_DIR: dataDirectory,
    SAAA_SMOKE_EXERCISE_SITUATION: "1",
  },
  stdout: "inherit",
  stderr: "inherit",
});

let ready = false;
try {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (existsSync(marker)) {
      ready = true;
      break;
    }
    if (application.exitCode !== null) break;
    await Bun.sleep(100);
  }
} finally {
  try {
    if (application.exitCode === null) application.kill();
    await application.exited;
  } finally {
    rmSync(dataDirectory, { recursive: true, force: true });
    if (existsSync(marker)) unlinkSync(marker);
  }
}
if (!ready) {
  console.error("Desktop smoke failed: the packaged frontend did not report ready within 10 seconds.");
  process.exit(1);
}
console.log(JSON.stringify({ desktop: "ready", executable, packagedCodex: process.platform === "darwin" }));
