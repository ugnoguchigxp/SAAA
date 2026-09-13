import { useEffect } from "react";
import { useCommittedCallback } from "./useCommittedCallback";
export function useIpcBoundaryError(onError: (message: string) => void) {
  const report = useCommittedCallback(onError);
  useEffect(() => {
    const handler = () =>
      report("IPC data could not be validated. Reload the application and retry.");
    window.addEventListener("saaa:ipc-boundary-error", handler);
    return () => window.removeEventListener("saaa:ipc-boundary-error", handler);
  }, [report]);
}
