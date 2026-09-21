import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";

export function useDelegatedReports(
  conversationId: string | null,
  onCommit: (conversationId: string, messageId: string) => void,
) {
  useEffect(() => {
    if (!conversationId) return;
    let stop = false;
    const unlisten = listen<{
      conversationId: string;
      messageId: string;
      reportRevision: number;
      cursor: number;
    }>("delegated-report-committed", (event) => {
      if (stop || event.payload.conversationId !== conversationId) return;
      onCommit(event.payload.conversationId, event.payload.messageId);
    });
    return () => {
      stop = true;
      void unlisten.then((fn) => fn());
    };
  }, [conversationId, onCommit]);
}
