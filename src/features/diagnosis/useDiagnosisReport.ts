import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import type { DiagnosisReport } from "../../lib/generated/diagnosis";
import { getDiagnosisReport, parseDiagnosisReport, runDiagnosis } from "./api";

export function useDiagnosisReport() {
  const [report, setReport] = useState<DiagnosisReport | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seen = useRef(-1);
  const rerunning = useRef(false);
  const mounted = useRef(true);
  const runningFromReport = useRef(false);
  const accept = useRef<(next: DiagnosisReport) => boolean>(() => false);
  accept.current = (next) => {
    if (!mounted.current || next.revision < seen.current) return false;
    seen.current = next.revision;
    runningFromReport.current = next.running;
    setReport(next);
    if (!rerunning.current) setRunning(next.running);
    return true;
  };

  useEffect(() => {
    mounted.current = true;
    let stop = false;
    void getDiagnosisReport()
      .then((next) => {
        if (!stop) accept.current(next);
      })
      .catch((cause: unknown) => {
        if (!stop && mounted.current)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
    const unlisten = listen<unknown>("diagnosis-updated", (event) => {
      if (stop) return;
      try {
        if (accept.current(parseDiagnosisReport(event.payload)) && mounted.current) setError(null);
      } catch (cause) {
        if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
      }
    });
    return () => {
      stop = true;
      mounted.current = false;
      void unlisten.then((unsubscribe) => unsubscribe());
    };
  }, []);

  async function rerun() {
    if (rerunning.current) return;
    rerunning.current = true;
    setRunning(true);
    setError(null);
    try {
      const next = await runDiagnosis();
      if (!mounted.current) return;
      if (next.revision < seen.current) {
        setRunning(runningFromReport.current);
        return;
      }
      seen.current = next.revision;
      runningFromReport.current = next.running;
      setReport(next);
      setRunning(next.running);
    } catch (cause) {
      if (mounted.current) {
        setRunning(false);
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      rerunning.current = false;
    }
  }

  return { report, running, error, rerun };
}
