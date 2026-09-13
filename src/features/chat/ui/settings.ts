import { useEffect, useSyncExternalStore } from "react";
import { uiApi } from "./api";
import { UiEnabledStore } from "./enabledStore";
const store = new UiEnabledStore(uiApi.enabled, uiApi.setEnabled);
export function useGenUiEnabled() {
  useEffect(() => {
    void store.load();
  }, []);
  return useSyncExternalStore(store.subscribe, store.snapshot);
}
export const setGenUiEnabled = store.set;
