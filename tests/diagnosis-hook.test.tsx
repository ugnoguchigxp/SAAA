import { afterEach, expect, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import type { DiagnosisReport, DiagnosisScope } from "../src/lib/generated/diagnosis";
import { installJsdom } from "./jsdomGlobals";
import { capability, report } from "./diagnosisFixtures";

const { useDiagnosisReport } = await import("../src/features/diagnosis/useDiagnosisReport");
type Backend = Parameters<typeof useDiagnosisReport>[0];

const listeners: Array<(payload: unknown) => void> = [];
const calls = { get: 0, run: [] as DiagnosisScope[] };
let getImpl: () => Promise<DiagnosisReport> = async () => snapshot(1);
let runImpl: (scope: DiagnosisScope) => Promise<DiagnosisReport> = async () => snapshot(1);

const backend: Backend = {
  listen: async (handler) => {
    listeners.push(handler);
    return () => {
      const index = listeners.indexOf(handler);
      if (index >= 0) listeners.splice(index, 1);
    };
  },
  get: async () => {
    calls.get += 1;
    return getImpl();
  },
  run: async (scope) => {
    calls.run.push(scope);
    return runImpl(scope);
  },
};

const snapshot = (revision: number, running = false): DiagnosisReport =>
  report("ready", [capability("storage", "ready")], { revision, running });

function Harness({
  apiRef,
}: {
  apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null>;
}) {
  apiRef.current = useDiagnosisReport(backend);
  return null;
}

let root: Root | null = null;
let restore: (() => void) | null = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  restore?.();
  restore = null;
  listeners.length = 0;
  calls.get = 0;
  calls.run = [];
  getImpl = async () => snapshot(1);
  runImpl = async () => snapshot(1);
});

async function mount() {
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  const apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null> = { current: null };
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(Harness, { apiRef })));
  await act(async () => {
    await Promise.resolve();
  });
  return apiRef;
}

test("hook loads the stored report once and follows progress events of a run", async () => {
  let finish: (value: DiagnosisReport) => void = () => undefined;
  runImpl = () =>
    new Promise<DiagnosisReport>((resolve) => {
      finish = resolve;
    });
  const api = await mount();
  expect(calls.get).toBe(1);
  expect(api.current?.loaded).toBe(true);
  expect(api.current?.report?.revision).toBe(1);

  let run: Promise<void> | undefined;
  await act(async () => {
    run = api.current!.run({ kind: "full" });
    await Promise.resolve();
  });
  expect(api.current?.running).toBe(true);
  await act(async () => {
    listeners[0]?.(snapshot(2, true));
  });
  expect(api.current?.report?.revision).toBe(2);
  expect(api.current?.running).toBe(true);
  finish(snapshot(2));
  await act(async () => {
    await run;
  });
  expect(api.current?.running).toBe(false);
  expect(api.current?.report?.running).toBe(false);
});

test("hook never replaces a newer report with an older run result", async () => {
  let resolveRun: (value: DiagnosisReport) => void = () => undefined;
  runImpl = () =>
    new Promise<DiagnosisReport>((resolve) => {
      resolveRun = resolve;
    });
  const api = await mount();
  let run: Promise<void> | undefined;
  await act(async () => {
    run = api.current!.run({ kind: "quick" });
    await Promise.resolve();
  });
  await act(async () => {
    listeners[0]?.(snapshot(3));
  });
  resolveRun(snapshot(2));
  await act(async () => {
    await run;
  });
  expect(api.current?.report?.revision).toBe(3);
});

test("a late partial snapshot of a finished revision does not reopen the run", async () => {
  const api = await mount();
  await act(async () => {
    listeners[0]?.(snapshot(2));
  });
  await act(async () => {
    listeners[0]?.(snapshot(2, true));
  });
  expect(api.current?.report?.running).toBe(false);
  expect(api.current?.running).toBe(false);
});

test("hook reports a malformed event and a failed run without crashing", async () => {
  runImpl = async () => {
    throw new Error("diagnosis failed");
  };
  const api = await mount();
  await act(async () => {
    listeners[0]?.({ revision: 9 });
  });
  expect(api.current?.error).toBeTruthy();
  expect(api.current?.report?.revision).toBe(1);
  await act(async () => {
    await api.current!.run({ kind: "quick" });
  });
  expect(api.current?.error).toContain("diagnosis failed");
  expect(api.current?.running).toBe(false);
});
