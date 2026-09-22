import { afterEach, expect, mock, test } from "bun:test";
import { act, type MutableRefObject } from "react";
import type { Root } from "react-dom/client";
import type { DiagnosisReport } from "../src/lib/generated/diagnosis";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { installJsdom } from "./jsdomGlobals";

const listeners: Array<(event: { payload: DiagnosisReport }) => void> = [];

mock.module("@tauri-apps/api/event", () => ({
  listen: async (_name: string, handler: (event: { payload: DiagnosisReport }) => void) => {
    listeners.push(handler);
    return () => {
      const index = listeners.indexOf(handler);
      if (index >= 0) listeners.splice(index, 1);
    };
  },
}));

const { useDiagnosisReport } = await import("../src/features/diagnosis/useDiagnosisReport");

function report(revision: number, running = false): DiagnosisReport {
  return {
    revision,
    startedAt: "2026-09-22T00:00:00Z",
    finishedAt: "2026-09-22T00:00:01Z",
    running,
    overall: running ? "running" : "ok",
    items: [],
  };
}

function Harness({
  apiRef,
}: {
  apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null>;
}) {
  apiRef.current = useDiagnosisReport();
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
});

test("diagnosis hook loads once, marks rerun as running, and applies events", async () => {
  resetTauriCoreMock();
  let finish = (_report: DiagnosisReport) => undefined;
  invokeImpl.handler = async (command) => {
    if (command === "get_diagnosis_report") return report(1);
    if (command === "run_diagnosis") {
      return new Promise<DiagnosisReport>((resolve) => {
        finish = resolve;
      });
    }
    return null;
  };
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  const apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null> = { current: null };
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(Harness, { apiRef })));
  await act(async () => {
    await Promise.resolve();
  });
  expect(invokeCalls.filter((call) => call.command === "get_diagnosis_report")).toHaveLength(1);
  expect(apiRef.current?.report?.revision).toBe(1);
  let rerun: Promise<void> | undefined;
  await act(async () => {
    rerun = apiRef.current!.rerun();
    await Promise.resolve();
  });
  expect(apiRef.current?.running).toBe(true);
  finish(report(2));
  await act(async () => {
    await rerun;
  });
  expect(apiRef.current?.report?.revision).toBe(2);
  expect(apiRef.current?.running).toBe(false);
  await act(async () => {
    listeners[0]?.({ payload: report(3) });
  });
  expect(apiRef.current?.report?.revision).toBe(3);
});

test("diagnosis hook keeps a newer report when an older fetch resolves later", async () => {
  resetTauriCoreMock();
  let resolveGet: (value: DiagnosisReport) => void = () => undefined;
  invokeImpl.handler = async (command) => {
    if (command === "get_diagnosis_report") {
      return new Promise<DiagnosisReport>((resolve) => {
        resolveGet = resolve;
      });
    }
    return report(1);
  };
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  const apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null> = { current: null };
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(Harness, { apiRef })));
  await act(async () => {
    listeners[0]?.({ payload: report(2) });
  });
  expect(apiRef.current?.report?.revision).toBe(2);
  resolveGet(report(1));
  await act(async () => {
    await Promise.resolve();
  });
  expect(apiRef.current?.report?.revision).toBe(2);
});

test("diagnosis hook ignores an older rerun result after a newer event", async () => {
  resetTauriCoreMock();
  let resolveRun: (value: DiagnosisReport) => void = () => undefined;
  invokeImpl.handler = async (command) => {
    if (command === "get_diagnosis_report") return report(1);
    if (command === "run_diagnosis") {
      return new Promise<DiagnosisReport>((resolve) => {
        resolveRun = resolve;
      });
    }
    return report(1);
  };
  restore = installJsdom().restore;
  const { createRoot } = await import("react-dom/client");
  const { createElement } = await import("react");
  const apiRef: MutableRefObject<ReturnType<typeof useDiagnosisReport> | null> = { current: null };
  root = createRoot(document.getElementById("root")!);
  await act(async () => root!.render(createElement(Harness, { apiRef })));
  await act(async () => {
    await Promise.resolve();
  });
  let rerun: Promise<void> | undefined;
  await act(async () => {
    rerun = apiRef.current!.rerun();
    await Promise.resolve();
  });
  await act(async () => {
    listeners[0]?.({ payload: report(3) });
  });
  resolveRun(report(2));
  await act(async () => {
    await rerun;
  });
  expect(apiRef.current?.report?.revision).toBe(3);
  expect(apiRef.current?.running).toBe(false);
});
