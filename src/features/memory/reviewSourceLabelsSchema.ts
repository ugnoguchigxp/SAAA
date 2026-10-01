import { z } from "zod";
export const reviewSourceLabelsSchema = z.array(
  z.object({ id: z.string(), version: z.number(), observedAt: z.number() }),
);
