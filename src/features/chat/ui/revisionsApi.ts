import { invoke } from "@tauri-apps/api/core";
import type { UiViewRevision } from "../../../lib/generated/generativeUi";

export const listUiViewRevisions = (viewId: string) =>
  invoke<UiViewRevision[]>("list_ui_view_revisions", { viewId });
