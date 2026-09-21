import type { UiNode } from "../../../lib/generated/generativeUi";

export type ArtifactWidth = 50 | 60 | 100;

/** Central policy boundary for future artifact-specific sizing. */
export function artifactWidthFor(_node: UiNode): ArtifactWidth {
  return 50;
}
