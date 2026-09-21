import { invoke } from "@tauri-apps/api/core";
import type {
  UiData,
  UiInstance,
  SavedView,
} from "../../../lib/generated/generativeUi";
import { uiMutationApi } from "./mutationsApi";
export { notifyUiHistoryChanged } from "./mutationsApi";
export type { UiData, UiInstance, SavedView };
export const uiApi = {
  enabled: () => invoke<boolean>("get_ui_enabled"),
  load: (instanceId: string, revision?: number) =>
    invoke<UiInstance>("get_ui_instance", { instanceId, revision }),
  query: (instanceId: string, source: string) =>
    invoke<UiData>("query_ui_source", { instanceId, source }),
  search: (query: string) => invoke<SavedView[]>("search_ui_views", { query }),
  ...uiMutationApi,
};
