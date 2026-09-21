import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const artifactSummarySchema = z.object({
  instanceId: z.string(),
  conversationId: z.string(),
  viewId: z.string(),
  revision: z.number(),
  summary: z.string(),
  name: z.string().nullable(),
  createdAt: z.string(),
});

export type ArtifactSummary = z.infer<typeof artifactSummarySchema>;

export const workApi = {
  artifacts: async () =>
    z.array(artifactSummarySchema).parse(await invoke("list_artifact_instances")),
};
