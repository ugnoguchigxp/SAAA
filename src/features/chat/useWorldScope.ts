import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { WorldContextStatus } from "../../lib/generated/runtimeEvent";
import { z } from "zod";
const ref = z.object({
  kind: z.enum(["user", "project", "task", "resource", "request"]),
  id: z.string(),
  relation: z.enum(["focus", "parent", "shared", "current"]),
});
const schema = z.object({
  choices: z.array(z.object({ key: z.string(), label: z.string(), refs: z.array(ref) })),
  messageScopes: z.record(z.string(), z.array(z.string())).default({}),
  latestScopeKeys: z.array(z.string()),
  latestProvider: z.string().nullable(),
  delivery: z.string().nullable(),
  omissionReason: z.string().nullable(),
});
export async function worldContextStatus(conversationId: string): Promise<WorldContextStatus> {
  return schema.parse(await invoke("world_context_status", { conversationId }));
}
export function resolveWorldSelection(
  status: WorldContextStatus,
  key: string,
): z.infer<typeof ref>[] | undefined {
  if (!key) return undefined;
  const selected = status.choices.find((choice) => choice.key === key);
  if (!selected) throw new Error("選択した対象が削除されました。対象を選び直してください。");
  return selected.refs.map((value) => ref.parse(value));
}
export function useWorldScope(conversationId: string | null, runId: string | null) {
  const [status, setStatus] = useState<WorldContextStatus | null>(null);
  const [selection, setSelection] = useState<Record<string, string>>({});
  useEffect(() => {
    let live = true;
    setStatus(null);
    if (conversationId)
      void worldContextStatus(conversationId)
        .then((value) => {
          if (live) setStatus(value);
        })
        .catch(() => {});
    return () => {
      live = false;
    };
  }, [conversationId, runId]);
  const key = conversationId ? (selection[conversationId] ?? "") : "";
  return {
    status,
    key,
    select: (key: string) => {
      if (conversationId) setSelection((previous) => ({ ...previous, [conversationId]: key }));
    },
    resolve: async (id: string) =>
      resolveWorldSelection(await worldContextStatus(id), selection[id] ?? ""),
  };
}
