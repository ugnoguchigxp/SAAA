import { reviewSourceLabelsSchema } from "./reviewSourceLabelsSchema";
import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";
export const retrospectiveSchema = z.object({
  mode: z.enum(["off", "preview", "apply"]),
  stages: z.array(
    z.object({
      stage: z.string(),
      reason: z.string(),
      count: z.number(),
      updatedAt: z.number(),
    }),
  ),
});
const candidatesSchema = z.array(
  z.object({
    id: z.number(),
    scope: z.string(),
    reason: z.string(),
    sources: reviewSourceLabelsSchema.default([]),
    candidates: z.array(
      z
        .object({ kind: z.string(), payload: z.unknown(), quote: z.string() })
        .passthrough(),
    ),
  }),
);
export const reviewApi = {
  setMode: (mode: "off" | "preview" | "apply") =>
    invoke<unknown>("set_world_review_mode", { mode }),
  candidates: async () =>
    candidatesSchema.parse(await invoke<unknown>("world_review_candidates")),
};
