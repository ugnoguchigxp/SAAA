import { createContext, useContext, useCallback, useRef, useSyncExternalStore } from 'react';
import { uiApi, type UiInstance } from './api';
import { uiQueries } from './queryCache';
import { uiStates } from './instanceState';
export type UiContextValue = { instance: UiInstance; conversationId: string; active: boolean; enabled: boolean };
export const UiContext = createContext<UiContextValue | null>(null);
export function useUiContext() { const value = useContext(UiContext); if (!value) throw new Error('Missing UI host'); return value; }
export function useUiData(source: string) {
  const { instance, conversationId, active, enabled } = useUiContext();
  const key = `${conversationId}:${source}`;
  const subscribe = useCallback((listener: () => void) => {
    if (instance.mode !== 'live' || !active || !enabled) return () => {};
    return uiQueries.subscribe(key, () => uiApi.query(instance.id, source), listener);
  }, [key, instance.id, instance.mode, source, active, enabled]);
  const snapshot = useSyncExternalStore(subscribe, () => uiQueries.snapshot(key));
  const previous = useRef(snapshot.data);
  if (snapshot.data) previous.current = snapshot.data;
  return instance.mode === 'snapshot' ? { data: instance.snapshots[source], loading: false } : { ...snapshot, data: snapshot.data ?? previous.current };
}
export function useUiField(key: string, fallback: string | number | boolean) {
  const { instance } = useUiContext();
  const state = useSyncExternalStore(useCallback(listener => uiStates.subscribe(instance.id, listener), [instance.id]), () => uiStates.get(instance.id).value);
  return [state[key] ?? fallback, (value: string | number | boolean) => uiStates.update(instance.id, key, value)] as const;
}
