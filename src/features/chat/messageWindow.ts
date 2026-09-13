import type { ConversationMessage } from "../../lib/contracts";
export const HISTORY_LIMIT = 150;
export function mergeMessageWindow(
  current: ConversationMessage[],
  incoming: ConversationMessage[],
  direction: "before" | "after",
) {
  const map = new Map(current.map((message) => [message.id, message]));
  incoming.forEach((message) => map.set(message.id, message));
  const all = [...map.values()].sort(
    (a, b) => Number(a.createdAt) - Number(b.createdAt) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
  return direction === "before" ? all.slice(0, HISTORY_LIMIT) : all.slice(-HISTORY_LIMIT);
}
