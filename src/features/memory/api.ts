import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const memorySourceRefSchema = z.object({ id: z.string() }).passthrough();

export const personalStateSnapshotSchema = z
  .object({
    enabled: z.boolean(),
    revision: z.number(),
    inputEpoch: z.number(),
    pendingCount: z.number(),
    pendingBytes: z.number(),
    contractReady: z.boolean(),
    contractReason: z.string().nullable(),
    items: z.array(
      z
        .object({
          id: z.string(),
          key: z.string(),
          status: z.string(),
          value: z.unknown(),
          source: z.array(memorySourceRefSchema),
        })
        .passthrough(),
    ),
    cleanup: z.array(
      z.object({ incarnation: z.string(), stage: z.string(), reason: z.string() }).passthrough(),
    ),
  })
  .passthrough();

export const personalSourcePageSchema = z.object({
  sources: z.array(
    z.object({
      id: z.string(),
      version: z.number(),
      sequence: z.number(),
      preview: z.string(),
      bytes: z.number(),
    }),
  ),
  nextSequence: z.number(),
  hasMore: z.boolean(),
});

export type PersonalStateSnapshot = z.infer<typeof personalStateSnapshotSchema>;
export type PersonalStateItem = PersonalStateSnapshot["items"][number];
export type PersonalSourcePage = z.infer<typeof personalSourcePageSchema>;

export const personalStateApi = {
  snapshot: async () =>
    personalStateSnapshotSchema.parse(await invoke<unknown>("personal_state_snapshot")),
  sources: async (afterSequence = 0) =>
    personalSourcePageSchema.parse(
      await invoke<unknown>("personal_source_page", { afterSequence }),
    ),
  forget: async (sourceId: string) =>
    personalStateSnapshotSchema.parse(
      await invoke<unknown>("forget_personal_source", { sourceId }),
    ),
};
