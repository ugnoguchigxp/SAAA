import { useEffect, useRef, useState } from "react";
import type { DiagnosisReport, DiagnosisScope } from "../../lib/generated/diagnosis";
import { getDiagnosisReport, parseDiagnosisReport, runDiagnosis } from "./api";

const CLOCK_MS = 30_000;

export type DiagnosisBackend = {
  listen: (handler: (payload: unknown) => void) => Promise<() => void>;
  get: typeof getDiagnosisReport;
  run: typeof runDiagnosis;
};

export const tauriBackend: DiagnosisBackend = {
  // Loaded on demand so tests that inject a backend never touch the Tauri event module.
  listen: async (handler) => {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<unknown>("diagnosis-updated", (event) => handler(event.payload));
  },
  get: getDiagnosisReport,
  run: runDiagnosis,
};

function message(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function useDiagnosisReport(backend: DiagnosisBackend = tauriBackend) {
  const [report, setReport] = useState<DiagnosisReport | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [requesting, setRequesting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const seen = useRef(-1);
  const mounted = useRef(true);
  const requests = useRef(0);

  const settled = useRef(false);
  const accept = (next: DiagnosisReport) => {
    if (!mounted.current || next.revision < seen.current) return false;
    // Snapshots of one revision are not ordered; a late partial one must not reopen a finished run.
    if (next.revision === seen.current && settled.current && next.running) return false;
    seen.current = next.revision;
    settled.current = !next.running;
    setReport(next);
    return true;
  };
  const acceptRef = useRef(accept);
  acceptRef.current = accept;

  useEffect(() => {
    mounted.current = true;
    let stop = false;
    const unlisten = backend
      .listen((payload) => {
        if (stop) return;
        try {
          if (acceptRef.current(parseDiagnosisReport(payload))) setError(null);
        } catch (cause) {
          if (mounted.current) setError(message(cause));
        }
      })
      .catch((cause) => {
        if (!stop) setError(message(cause));
        return () => undefined;
      });
    // Loading the stored report must not depend on the event subscription succeeding.
    void backend
      .get()
      .then((latest) => {
        if (!stop) acceptRef.current(latest);
      })
      .catch((cause) => {
        if (!stop) setError(message(cause));
      })
      .finally(() => {
        if (!stop) setLoaded(true);
      });
    const clock = setInterval(() => setNow(Date.now()), CLOCK_MS);
    return () => {
      stop = true;
      mounted.current = false;
      clearInterval(clock);
      void unlisten.then((unsubscribe) => unsubscribe());
    };
  }, [backend]);

  async function run(scope: DiagnosisScope) {
    requests.current += 1;
    setRequesting(true);
    setError(null);
    try {
      acceptRef.current(await backend.run(scope));
    } catch (cause) {
      if (mounted.current) setError(message(cause));
    } finally {
      requests.current -= 1;
      if (mounted.current && requests.current === 0) setRequesting(false);
      if (mounted.current) setNow(Date.now());
    }
  }

  return { report, loaded, running: requesting || report?.running === true, error, now, run };
}
