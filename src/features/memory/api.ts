import { personalSourcePageSchema } from "./sourcePageSchema";
import { maintenanceSchema } from "./maintenanceSchema";
import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const memorySourceRefSchema = z.object({ id: z.string() }).passthrough();

const memoryItemSchema = z
  .object({
    id: z.string(),
    key: z.string(),
    status: z.string(),
    value: z.unknown(),
    source: z.array(memorySourceRefSchema),
  })
  .passthrough();

export const personalStateSnapshotSchema = z
  .object({
    enabled: z.boolean(),
    enabledOverride: z.boolean().default(false),
    revision: z.number(),
    inputEpoch: z.number(),
    pendingCount: z.number(),
    pendingBytes: z.number(),
    contractReady: z.boolean(),
    contractReason: z.string().nullable(),
    maintenance: maintenanceSchema,
    items: z.array(memoryItemSchema),
    worldItems: z.array(memoryItemSchema).default([]),
    cleanup: z.array(
      z.object({ incarnation: z.string(), stage: z.string(), reason: z.string() }).passthrough(),
    ),
  })
  .passthrough();

export { personalSourcePageSchema } from "./sourcePageSchema";

export type PersonalStateSnapshot = z.infer<typeof personalStateSnapshotSchema>;
export type PersonalStateItem = PersonalStateSnapshot["items"][number];
export type PersonalSourcePage = z.infer<typeof personalSourcePageSchema>;

export const personalStateApi = {
  setEnabled: async (enabled: boolean) =>
    personalStateSnapshotSchema.parse(
      await invoke<unknown>("set_personal_state_enabled", { enabled }),
    ),
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
