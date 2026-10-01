import { z } from "zod";

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
