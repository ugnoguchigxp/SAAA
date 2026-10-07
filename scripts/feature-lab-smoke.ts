import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdtempSync } from "node:fs";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { createServer } from "vite";

import { browserText } from "./feature-lab-smoke-browser";
import { smokeDiagnostics, smokeExitCode } from "./feature-lab-smoke-reap";
import { featureLabViteConfig } from "./feature-lab-vite.config";
import { cleanupOwnedResources, trackOwnedProcesses } from "./verification-process";

const root = join(import.meta.dir, "..");
const binary = join(root, "src-tauri/target/debug/saaa-feature-lab");

function chromePath(): string | null {
  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

function listenerOpen(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    const done = (open: boolean) => {
      socket.destroy();
      resolve(open);
    };
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
    socket.setTimeout(500, () => done(true));
  });
}

async function main(): Promise<number> {
  const chrome = chromePath();
  if (!chrome) {
    console.error("feature-lab smoke requires a local Chrome or Chromium");
    return 1;
  }
  if (!existsSync(binary)) {
    console.error("feature-lab host binary is missing");
    return 1;
  }
  const tracker = trackOwnedProcesses();
  const directory = mkdtempSync(join(tmpdir(), "saaa-lab-smoke-"));
  const token = randomBytes(32).toString("hex");
  const host = spawn(binary, [], { detached: true, stdio: ["pipe", "pipe", "inherit"] });
  tracker.register(host.pid ?? 0, "host", true);
  const tracking = setInterval(() => tracker.collect(), 50);
  const directories = [directory];
  let ui: { close: () => Promise<void> } | undefined;
  let pageOk = false;
  let signal: 0 | 130 | 143 = 0;
  const abort = new AbortController();
  const interrupt = () => {
    signal = 130;
    abort.abort();
  };
  const terminate = () => {
    signal = 143;
    abort.abort();
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  host.stdin.on("error", () => {});
  try {
    host.stdin.write(
      `${JSON.stringify({
        databasePath: join(directory, "lab.sqlite"),
        allowedOrigin: "http://127.0.0.1:1422",
        sessionToken: token,
        provider: "fixture",
        larmEndpoint: "http://127.0.0.1:9/",
        larmToken: null,
      })}\n`,
    );
    const readyLine = await readReady(host, abort.signal);
    if (!signal) {
      if (readyLine.includes(token)) throw new Error("feature-lab ready output exposed a secret");
      const ready = JSON.parse(readyLine) as { ready?: unknown; port?: unknown };
      if (ready.ready !== true || typeof ready.port !== "number") {
        throw new Error("feature-lab ready output was not a port");
      }
      const server = await createServer(
        featureLabViteConfig({ token, apiTarget: `http://127.0.0.1:${ready.port}` }),
      );
      ui = server;
      abort.signal.throwIfAborted();
      await server.listen();
      abort.signal.throwIfAborted();
      // Isolate the browser controller so cancellation cannot leave a pending launch behind.
      // Chrome's temporary profile is inside our already-owned temporary directory.
      const worker = spawn(process.execPath, [import.meta.path, "--browser-worker", chrome], {
        detached: true,
        stdio: ["ignore", "pipe", "inherit"],
        env: { ...process.env, TMPDIR: directory, TMP: directory, TEMP: directory },
      });
      tracker.register(worker.pid ?? 0, "browser-controller", true);
      const page = JSON.parse(await readReady(worker, abort.signal, 60_000)) as {
        text?: string;
        pid?: number;
        error?: string;
      };
      if (page.pid) tracker.register(page.pid, "chrome", true);
      if (page.error) throw new Error(page.error);
      pageOk = page.text === "ok";
      if (!pageOk) throw new Error(`feature-lab browser smoke failed: ${page.text}`);
    }
  } catch (cause) {
    if (!signal) console.error(cause instanceof Error ? cause.message : cause);
  }
  host.stdin.end();
  const finish = await cleanupOwnedResources({
    collect: tracker.collect,
    directories,
    trackingErrors: tracker.errors,
    closeVite: () => ui?.close() ?? Promise.resolve(),
    viteListening: () => (ui ? listenerOpen(1422) : Promise.resolve(false)),
  });
  clearInterval(tracking);
  process.off("SIGINT", interrupt);
  process.off("SIGTERM", terminate);
  if (finish.directoriesRemaining.length)
    finish.errors.push(`temporary directories remain: ${finish.directoriesRemaining.join(", ")}`);
  const text = smokeDiagnostics(finish);
  if (text) console.error(text);
  return smokeExitCode({ signal, pageOk, finish });
}

function readReady(host: ChildProcess, signal: AbortSignal, timeoutMs = 30_000): Promise<string> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: host.stdout! });
    let settled = false;
    let timer: ReturnType<typeof setTimeout>;
    const finish = (callback: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      lines.close();
      signal.removeEventListener("abort", stop);
      host.off("error", failed);
      host.off("exit", exited);
      callback();
    };
    timer = setTimeout(
      () => finish(() => reject(new Error("feature-lab host did not become ready"))),
      timeoutMs,
    );
    lines.once("line", (line) => finish(() => resolve(line)));
    const stop = () => finish(() => reject(new Error("feature-lab smoke interrupted")));
    const failed = () => finish(() => reject(new Error("feature-lab process could not start")));
    const exited = () => finish(() => reject(new Error("feature-lab process exited before ready")));
    signal.addEventListener("abort", stop, { once: true });
    host.once("error", failed);
    host.once("exit", exited);
    if (signal.aborted) stop();
  });
}

if (import.meta.main) {
  if (process.argv[2] === "--browser-worker") {
    // Keep the controller alive during graceful termination so its detached Chrome
    // remains discoverable by the owner. The owner enforces the force-kill deadline.
    process.on("SIGTERM", () => {});
    process.on("SIGINT", () => {});
  }
  const task =
    process.argv[2] === "--browser-worker"
      ? browserText(
          process.argv[3],
          "http://127.0.0.1:1422/scripts/feature-lab-smoke-page.html",
        ).then(
          (page) => {
            console.log(JSON.stringify(page));
            return 0;
          },
          (cause: unknown) => {
            const error = cause as { pid?: number };
            console.log(
              JSON.stringify({ error: "feature-lab browser controller failed", pid: error.pid }),
            );
            return 1;
          },
        )
      : main();
  task
    .then((code) => process.exit(code))
    .catch((error: unknown) => {
      console.error(error instanceof Error ? error.message : error);
      process.exit(1);
    });
}
