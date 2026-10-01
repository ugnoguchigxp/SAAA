import { z } from "zod";

export const maintenanceSchema = z
  .object({
    reason: z.string(),
    failures: z.number(),
    nextAttemptAt: z.number(),
    updatedAt: z.number(),
    work: z
      .object({
        queued: z.number(),
        running: z.number(),
        failed: z.number(),
        held: z.number(),
        worldPending: z.number(),
        deferred: z.number(),
      })
      .optional(),
  })
  .optional();
