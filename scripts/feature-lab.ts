import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { createServer } from "vite";

import { featureLabViteConfig } from "./feature-lab-vite.config";

const READY_MS = 30_000;
const GRACE_MS = 10_000;

export type HostProcess = {
  writeStdin: (text: string) => void;
  closeStdin: () => void;
  readStdoutLine: () => Promise<string>;
  kill: (signal?: NodeJS.Signals) => void;
  exited: Promise<number | null>;
};

export type FeatureLabDeps = {
  root: string;
  databasePath: string;
  provider: "fixture" | "larm";
  larmEndpoint: string;
  larmToken: string | null;
  replaceRoute?: boolean;
  token: string;
  build: () => Promise<void>;
  binary: string;
  spawnHost: (binary: string) => HostProcess;
  listen: (input: { token: string; apiTarget: string }) => Promise<{ close: () => Promise<void> }>;
  log: (line: string) => void;
  signal: AbortSignal;
  readyMs?: number;
  graceMs?: number;
};

export async function runFeatureLab(deps: FeatureLabDeps): Promise<void> {
  await deps.build();
  const host = deps.spawnHost(deps.binary);
  let ui: { close: () => Promise<void> } | undefined;
  try {
    const config = {
      databasePath: deps.databasePath,
      allowedOrigin: "http://127.0.0.1:1422",
      sessionToken: deps.token,
      provider: deps.provider,
      larmEndpoint: deps.larmEndpoint,
      larmToken: deps.larmToken,
      replaceRoute: deps.replaceRoute === true,
    };
    host.writeStdin(`${JSON.stringify(config)}\n`);
    const line = await Promise.race([
      host.readStdoutLine(),
      rejectAfter(deps.readyMs ?? READY_MS, "feature-lab host did not become ready"),
      abortError(deps.signal),
    ]);
    if (line.includes(deps.token) || (deps.larmToken && line.includes(deps.larmToken))) {
      throw new Error("feature-lab ready output exposed a secret");
    }
    let ready: { ready?: unknown; port?: unknown };
    try {
      ready = JSON.parse(line) as { ready?: unknown; port?: unknown };
    } catch {
      throw new Error("feature-lab ready output was not a port");
    }
    if (ready.ready !== true || typeof ready.port !== "number") {
      throw new Error("feature-lab ready output was not a port");
    }
    ui = await deps.listen({
      token: deps.token,
      apiTarget: `http://127.0.0.1:${ready.port}`,
    });
    const provider = deps.provider === "larm" ? "larm" : "fixture";
    deps.log(
      `feature-lab listening at http://127.0.0.1:1422/scripts/feature-lab-preview.html?transport=http&provider=${provider}`,
    );
    await abortError(deps.signal).catch(() => undefined);
  } catch (error) {
    host.kill("SIGKILL");
    throw error;
  } finally {
    await ui?.close().catch(() => undefined);
    host.closeStdin();
    const exited = await Promise.race([
      host.exited.then(() => true),
      delay(deps.graceMs ?? GRACE_MS).then(() => false),
    ]);
    if (!exited) host.kill("SIGKILL");
  }
}

function rejectAfter(ms: number, message: string): Promise<never> {
  return new Promise((_, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    timer.unref?.();
  });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    timer.unref?.();
  });
}

function abortError(signal: AbortSignal): Promise<never> {
  if (signal.aborted) return Promise.reject(signal.reason ?? new Error("aborted"));
  return new Promise((_, reject) => {
    signal.addEventListener("abort", () => reject(signal.reason ?? new Error("aborted")), {
      once: true,
    });
  });
}

function childHost(binary: string): HostProcess {
  const child = spawn(binary, [], { stdio: ["pipe", "pipe", "inherit"] });
  const lines = createInterface({ input: child.stdout });
  const pending: string[] = [];
  let waiter: ((line: string) => void) | undefined;
  let closed = false;
  let rejectWaiter: ((error: Error) => void) | undefined;
  const outputEnded = () => new Error("feature-lab host output ended");
  lines.on("line", (line) => {
    if (waiter) {
      const resolve = waiter;
      waiter = undefined;
      rejectWaiter = undefined;
      resolve(line);
      return;
    }
    pending.push(line);
  });
  lines.on("close", () => {
    closed = true;
    waiter = undefined;
    rejectWaiter?.(outputEnded());
    rejectWaiter = undefined;
  });
  return {
    writeStdin: (text) => {
      child.stdin.write(text);
    },
    closeStdin: () => {
      child.stdin.end();
    },
    readStdoutLine: () =>
      new Promise((resolve, reject) => {
        const next = pending.shift();
        if (next !== undefined) resolve(next);
        else if (closed) reject(outputEnded());
        else {
          waiter = resolve;
          rejectWaiter = reject;
        }
      }),
    kill: (signal = "SIGTERM") => child.kill(signal),
    exited: new Promise((resolve) => {
      child.on("exit", (code) => resolve(code));
    }),
  };
}

async function main(): Promise<void> {
  const root = join(import.meta.dir, "..");
  const token = randomBytes(32).toString("hex");
  const explicit = explicitLabConfig(process.argv.slice(2));
  const databasePath = join(
    root,
    "src-tauri/target/feature-lab",
    labDatabaseName(explicit.provider),
  );
  mkdirSync(join(root, "src-tauri/target/feature-lab"), { recursive: true });
  const controller = new AbortController();
  process.on("SIGINT", () => controller.abort());
  process.on("SIGTERM", () => controller.abort());
  await runFeatureLab({
    root,
    databasePath,
    provider: explicit.provider,
    larmEndpoint: explicit.larmEndpoint,
    larmToken: explicit.larmToken,
    replaceRoute: explicit.replaceRoute,
    token,
    build: async () => {
      const build = spawn(
        "bun",
        ["run", "--silent", "verify", "build", "--package", "services/feature-lab"],
        {
          cwd: root,
          stdio: "inherit",
        },
      );
      const code = await new Promise<number | null>((resolve) => {
        build.on("exit", resolve);
      });
      if (code !== 0) throw new Error("feature-lab build failed");
    },
    binary: join(root, "src-tauri/target/debug/saaa-feature-lab"),
    spawnHost: childHost,
    listen: async ({ token: session, apiTarget }) => {
      const server = await createServer(featureLabViteConfig({ token: session, apiTarget }));
      await server.listen();
      return { close: () => server.close() };
    },
    log: (line) => console.error(line),
    signal: controller.signal,
  });
}

export function labDatabaseName(provider: "fixture" | "larm"): string {
  return provider === "larm" ? "lab-larm.sqlite" : "lab-fixture.sqlite";
}

function explicitLabConfig(argv: string[]): {
  provider: "fixture" | "larm";
  larmEndpoint: string;
  larmToken: string | null;
  replaceRoute: boolean;
} {
  const replaceRoute = argv.includes("--replace-route");
  const flag = argv.indexOf("--config");
  if (flag < 0) {
    return {
      provider: "fixture",
      larmEndpoint: "http://127.0.0.1:9/",
      larmToken: null,
      replaceRoute,
    };
  }
  const path = argv[flag + 1];
  if (!path) throw new Error("feature-lab --config needs a file path");
  const parsed = JSON.parse(readFileSync(path, "utf8")) as {
    provider?: unknown;
    larmEndpoint?: unknown;
    larmToken?: unknown;
  };
  return {
    provider: parsed.provider === "larm" ? "larm" : "fixture",
    larmEndpoint:
      typeof parsed.larmEndpoint === "string" ? parsed.larmEndpoint : "http://127.0.0.1:9/",
    larmToken: typeof parsed.larmToken === "string" ? parsed.larmToken : null,
    replaceRoute,
  };
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "feature-lab failed");
    process.exit(1);
  });
}
