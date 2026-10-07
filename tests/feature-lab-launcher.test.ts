import { expect, test } from "bun:test";

import {
  labDatabaseName,
  runFeatureLab,
  type FeatureLabDeps,
  type HostProcess,
} from "../scripts/feature-lab";
import { featureLabViteConfig, labRequestDecision } from "../scripts/feature-lab-vite.config";

const token = "0123456789abcdef0123456789abcdef";

function deps(overrides: Partial<FeatureLabDeps> = {}): FeatureLabDeps {
  return {
    root: "/tmp",
    databasePath: "/tmp/lab.sqlite",
    provider: "fixture",
    larmEndpoint: "http://127.0.0.1:9/",
    larmToken: null,
    token,
    build: async () => {},
    binary: "saaa-feature-lab",
    spawnHost: () => {
      throw new Error("spawned");
    },
    listen: async () => ({ close: async () => {} }),
    log: () => {},
    signal: new AbortController().signal,
    readyMs: 40,
    graceMs: 20,
    ...overrides,
  };
}

function readyHost(line?: string): {
  process: HostProcess;
  state: { stdin: string; killed: string[]; closed: boolean };
} {
  const state = { stdin: "", killed: [] as string[], closed: false };
  let finish: (code: number | null) => void = () => {};
  const exited = new Promise<number | null>((resolve) => {
    finish = resolve;
  });
  const process: HostProcess = {
    writeStdin: (text) => {
      state.stdin += text;
    },
    closeStdin: () => {
      state.closed = true;
      finish(0);
    },
    readStdoutLine: () => (line === undefined ? new Promise(() => {}) : Promise.resolve(line)),
    kill: (signal) => {
      state.killed.push(signal ?? "SIGTERM");
      finish(null);
    },
    exited,
  };
  return { process, state };
}

test("launcher does not start the host when the build fails", async () => {
  let spawned = 0;
  await expect(
    runFeatureLab(
      deps({
        build: async () => {
          throw new Error("build failed");
        },
        spawnHost: () => {
          spawned += 1;
          throw new Error("spawned");
        },
      }),
    ),
  ).rejects.toThrow("build failed");
  expect(spawned).toBe(0);
});

test("launcher rejects a missing ready line", async () => {
  const hung = readyHost();
  await expect(runFeatureLab(deps({ spawnHost: () => hung.process, readyMs: 30 }))).rejects.toThrow(
    "feature-lab host did not become ready",
  );
  expect(hung.state.killed).toEqual(["SIGKILL"]);
});

test("launcher rejects a bad ready line and a secret in ready output", async () => {
  const bad = readyHost("not-json");
  await expect(runFeatureLab(deps({ spawnHost: () => bad.process }))).rejects.toThrow(
    "feature-lab ready output was not a port",
  );
  expect(bad.state.killed).toEqual(["SIGKILL"]);

  const leaked = readyHost(`{"ready":true,"port":9,"sessionToken":"${token}"}`);
  await expect(runFeatureLab(deps({ spawnHost: () => leaked.process }))).rejects.toThrow(
    "feature-lab ready output exposed a secret",
  );
  expect(leaked.state.stdin).toContain(token);
  expect(leaked.state.killed).toEqual(["SIGKILL"]);
});

test("launcher stops both processes on abort without logging the token", async () => {
  const host = readyHost('{"ready":true,"port":9}');
  const logs: string[] = [];
  let closed = false;
  const controller = new AbortController();
  await runFeatureLab(
    deps({
      spawnHost: () => host.process,
      signal: controller.signal,
      log: (line) => logs.push(line),
      listen: async () => {
        controller.abort();
        return {
          close: async () => {
            closed = true;
          },
        };
      },
    }),
  );
  expect(closed).toBe(true);
  expect(host.state.closed).toBe(true);
  expect(logs.join("")).not.toContain(token);
  expect(logs.join("")).toContain("provider=fixture");
});

test("launcher does not listen when the host process fails to spawn", async () => {
  let listened = 0;
  await expect(
    runFeatureLab(
      deps({
        spawnHost: () => {
          throw new Error("port in use");
        },
        listen: async () => {
          listened += 1;
          return { close: async () => {} };
        },
      }),
    ),
  ).rejects.toThrow("port in use");
  expect(listened).toBe(0);
});

test("launcher kills the host when the preview port is already taken", async () => {
  const host = readyHost('{"ready":true,"port":9}');
  await expect(
    runFeatureLab(
      deps({
        spawnHost: () => host.process,
        listen: async () => {
          throw new Error("port in use");
        },
      }),
    ),
  ).rejects.toThrow("port in use");
  expect(host.state.killed).toEqual(["SIGKILL"]);
});

test("launcher kills the host when it exits before ready", async () => {
  const state = { stdin: "", killed: [] as string[], closed: false };
  const process: HostProcess = {
    writeStdin: (text) => {
      state.stdin += text;
    },
    closeStdin: () => {
      state.closed = true;
    },
    readStdoutLine: () => Promise.reject(new Error("feature-lab host output ended")),
    kill: (signal) => {
      state.killed.push(signal ?? "SIGTERM");
    },
    exited: Promise.resolve(1),
  };
  await expect(runFeatureLab(deps({ spawnHost: () => process }))).rejects.toThrow(
    "feature-lab host output ended",
  );
  expect(state.killed).toEqual(["SIGKILL"]);
});

test("vite document cookie is limited to the preview document", () => {
  expect(
    labRequestDecision({
      method: "GET",
      path: "/scripts/feature-lab-preview.html",
      host: "127.0.0.1:1422",
      dest: "document",
      mode: "navigate",
    }),
  ).toBe("document");
  expect(
    labRequestDecision({
      method: "POST",
      path: "/api/v1/media/runs",
      host: "127.0.0.1:1422",
      origin: "http://127.0.0.1:1422",
      site: "same-origin",
    }),
  ).toBe("proxy");
  expect(
    labRequestDecision({
      method: "GET",
      path: "/api/v1/media/runs",
      host: "127.0.0.1:1422",
      site: "same-origin",
    }),
  ).toBe("proxy");
  expect(
    labRequestDecision({
      method: "POST",
      path: "/api/v1/media/runs",
      host: "127.0.0.1:1422",
    }),
  ).toBe("forbid");
  expect(
    labRequestDecision({
      method: "GET",
      path: "/scripts/feature-lab-preview.html",
      host: "127.0.0.1:1422",
      dest: "iframe",
    }),
  ).toBe("forbid");
  expect(
    labRequestDecision({
      method: "GET",
      path: "/scripts/feature-lab-preview.html",
      host: "127.0.0.1:1422",
      site: "cross-site",
    }),
  ).toBe("forbid");
  const plain = featureLabViteConfig();
  const proxied = featureLabViteConfig({ token, apiTarget: "http://127.0.0.1:9" });
  expect(plain.server?.proxy).toBeUndefined();
  expect(proxied.server?.proxy).toBeDefined();
  expect(JSON.stringify(plain)).not.toContain(token);
  expect(
    labRequestDecision({
      method: "GET",
      path: "/scripts/feature-lab-preview.html",
      dest: "document",
      mode: "navigate",
    }),
  ).toBe("forbid");
});

test("fixture and larm use separate databases, and route replacement is explicit", async () => {
  expect(labDatabaseName("fixture")).toBe("lab-fixture.sqlite");
  expect(labDatabaseName("larm")).toBe("lab-larm.sqlite");
  const host = readyHost('{"ready":true,"port":9}');
  const controller = new AbortController();
  const pending = runFeatureLab(
    deps({
      spawnHost: () => host.process,
      replaceRoute: true,
      signal: controller.signal,
    }),
  );
  controller.abort();
  await pending.catch(() => undefined);
  expect(host.state.stdin).toContain('"replaceRoute":true');
});
