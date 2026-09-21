import { invoke } from "@tauri-apps/api/core";
import type { UiInstance } from "../../../lib/generated/generativeUi";

export const uiMutationApi = {
  setEnabled: (enabled: boolean) => invoke<void>("set_ui_enabled", { enabled }),
  state: (
    instanceId: string,
    expectedVersion: number,
    value: UiInstance["state"],
  ) =>
    invoke<number>("save_ui_instance_state", {
      instanceId,
      expectedVersion,
      value,
    }),
  save: (instanceId: string, name: string, description: string) =>
    invoke("publish_ui_view", {
      input: { instanceId, name, description, tags: [] },
    }),
  archive: (viewId: string) => invoke<void>("archive_ui_view", { viewId }),
  open: (conversationId: string, viewId: string) =>
    invoke("open_ui_view", { conversationId, viewId }),
  snapshot: (instanceId: string) => invoke("snapshot_ui_view", { instanceId }),
  cancel: (instanceId: string, targetId: string, requestId: string) =>
    invoke("cancel_ui_run", { instanceId, targetId, requestId }),
};

export function notifyUiHistoryChanged(conversationId: string) {
  window.dispatchEvent(
    new CustomEvent("saaa:ui-history", { detail: conversationId }),
  );
}
