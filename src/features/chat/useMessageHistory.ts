import { useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { MessageHistoryStore, type MessagePage } from "./messageHistoryStore";
export function useMessageHistory() {
  const [store] = useState(
    () =>
      new MessageHistoryStore((conversationId, cursor, direction) =>
        invoke<MessagePage>("list_message_window", { conversationId, cursor, direction }),
      ),
  );
  const snapshot = useSyncExternalStore(store.subscribe, store.snapshot);
  return {
    ...snapshot,
    setMessages: store.setMessages,
    reset: store.reset,
    latest: store.latest,
    load: store.load,
    isBrowsingOlder: store.isBrowsingOlder,
  };
}
