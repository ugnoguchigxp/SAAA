import { invoke } from '@tauri-apps/api/core';
import type { UiData, UiInstance, SavedView } from '../../../lib/generated/generativeUi';
export type { UiData, UiInstance, SavedView };
export const uiApi = {
  enabled: () => invoke<boolean>('get_ui_enabled'),
  setEnabled: (enabled: boolean) => invoke<void>('set_ui_enabled', { enabled }),
  load: (instanceId: string) => invoke<UiInstance>('get_ui_instance', { instanceId }),
  query: (instanceId: string, source: string) => invoke<UiData>('query_ui_source', { instanceId, source }),
  state: (instanceId: string, expectedVersion: number, value: UiInstance['state']) => invoke<number>('save_ui_instance_state', { instanceId, expectedVersion, value }),
  save: (instanceId: string, name: string, description: string) => invoke('publish_ui_view', { input: { instanceId, name, description, tags: [] } }),
  archive: (viewId: string) => invoke<void>('archive_ui_view', { viewId }),
  search: (query: string) => invoke<SavedView[]>('search_ui_views', { query }),
  open: (conversationId: string, viewId: string) => invoke('open_ui_view', { conversationId, viewId }),
  snapshot: (instanceId: string) => invoke('snapshot_ui_view', { instanceId }),
  cancel: (instanceId: string, targetId: string, requestId: string) => invoke('cancel_ui_run', { instanceId, targetId, requestId }),
};
export function notifyUiHistoryChanged(conversationId: string) {
  window.dispatchEvent(new CustomEvent('saaa:ui-history', { detail: conversationId }));
}
