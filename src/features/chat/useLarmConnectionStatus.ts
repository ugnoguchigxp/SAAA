import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ConnectionStatus = { state: string; message: string };

/** The read-only IPC status poll never touches the LARM control plane or idle timer. */
export function useLarmConnectionStatus(conversationId: string | null, active: boolean) {
  const [status, setStatus] = useState<ConnectionStatus | null>(null);
  useEffect(() => {
    if (!active || !conversationId) {
      setStatus(null);
      return;
    }
    let mounted = true;
    const poll = () => {
      void invoke<ConnectionStatus | null>("larm_voice_connection_status", {
        conversationId,
      }).then(
        (value) => { if (mounted) setStatus(value); },
        () => { if (mounted) setStatus(null); },
      );
    };
    poll();
    const timer = window.setInterval(poll, 1_000);
    return () => {
      mounted = false;
      window.clearInterval(timer);
    };
  }, [active, conversationId]);
  return status;
}
