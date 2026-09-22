import { invoke } from "@tauri-apps/api/core";
import type {
  ArtifactPreviewDescriptor,
  MountArtifactPreviewInput,
  PrepareArtifactPreviewInput,
  ReleaseArtifactPreviewInput,
} from "../../../lib/generated/artifactPreview";

export const INTERACTIVE_HTML_FIXTURE: PrepareArtifactPreviewInput = {
  artifactId: "artifact.fixture.interactive-html",
  revisionId: "rev.fixture.interactive-html.v1",
};

export const artifactPreviewApi = {
  prepare: (input: PrepareArtifactPreviewInput) =>
    invoke<ArtifactPreviewDescriptor>("prepare_artifact_preview", { input }),
  mount: (input: MountArtifactPreviewInput) => invoke<void>("mount_artifact_preview", { input }),
  release: (input: ReleaseArtifactPreviewInput) =>
    invoke<void>("release_artifact_preview", { input }),
};
