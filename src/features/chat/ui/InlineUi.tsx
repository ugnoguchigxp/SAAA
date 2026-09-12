import { useCallback, useEffect, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { uiApi, notifyUiHistoryChanged, type UiInstance } from './api';
import { UiContext } from './context';
import { SemanticRenderer } from './SemanticRenderer';
import { uiStates } from './instanceState';
import { useUiVisibility } from './visibility';
import { useGenUiEnabled } from './settings';
import './ui.css';
import { UiBoundary } from './UiBoundary';
function LoadedUi({ instance, conversationId, active, enabled }: { instance: UiInstance; conversationId: string; active: boolean; enabled: boolean }) {
  const { t } = useTranslation(); const draft = useSyncExternalStore(useCallback(listener => uiStates.subscribe(instance.id, listener), [instance.id]), () => uiStates.get(instance.id).value);
  const name = String(draft.__viewName ?? instance.name ?? '');
  const setName = (value: string) => uiStates.update(instance.id, '__viewName', value);
  const [saving, setSaving] = useState(false); const [outcome, setOutcome] = useState('');
  const stateError = useSyncExternalStore(useCallback(listener => uiStates.subscribe(instance.id, listener), [instance.id]), () => uiStates.get(instance.id).error);
  async function save() { setSaving(true); try { await uiApi.save(instance.id, name, instance.summary); setOutcome('genui.savedDone'); } catch { setOutcome('genui.operationFailed'); } finally { setSaving(false); } }
  async function snapshot() { setSaving(true); try { await uiApi.snapshot(instance.id); notifyUiHistoryChanged(conversationId); } catch { setOutcome('genui.operationFailed'); } finally { setSaving(false); } }
  return <UiContext.Provider value={{ instance, conversationId, active, enabled }}>
    <header className="ui-header"><strong>{instance.name ?? instance.summary}</strong><span>{t(instance.mode === 'snapshot' ? 'genui.snapshot' : 'genui.live')} · v{instance.revision}</span></header>
    {!enabled && instance.mode === 'live' && <p>{t('genui.disabled')}</p>}
    <SemanticRenderer node={instance.node} />
    {stateError && <p role="alert">{t('genui.stateFailed')} <button onClick={() => void uiStates.flush(instance.id)}>{t('genui.retry')}</button></p>}
    <footer className="ui-footer"><form onSubmit={event => { event.preventDefault(); void save(); }}><label>{t('genui.name')}<input value={name} maxLength={120} onChange={event => setName(event.target.value)} /></label><button disabled={!enabled || saving || !name.trim()}>{t('genui.save')}</button></form>{instance.mode === 'live' && <button disabled={!enabled || saving || !active} onClick={() => void snapshot()}>{t('genui.snapshotAction')}</button>}{outcome && <p role="status">{t(outcome)}</p>}<small>{t('genui.editHint')}</small></footer>
  </UiContext.Provider>;
}
export default function InlineUi({ instanceId, summary, conversationId }: { instanceId: string; summary: string; conversationId: string }) {
  const { t } = useTranslation(); const [instance, setInstance] = useState<UiInstance>(); const [failed, setFailed] = useState(false); const [retry, setRetry] = useState(0);
  const { ref, active } = useUiVisibility(); const enabled = useGenUiEnabled();
  useEffect(() => {
    let disposed = false; let release: (() => void) | undefined;
    setFailed(false); setInstance(undefined);
    void uiApi.load(instanceId).then(value => {
      if (disposed) return;
      if (value.libraryVersion !== 1) throw new Error("Unsupported UI version");
      release = uiStates.retain(value); setInstance(value);
    }).catch(() => { if (!disposed) setFailed(true); });
    return () => { disposed = true; release?.(); };
  }, [instanceId, retry]);
  const fallback = <div><p>{summary}</p><p role="status">{t('genui.unavailable')}</p><button onClick={() => setRetry(value => value + 1)}>{t('genui.retry')}</button></div>;
  return <div className="inline-ui" ref={ref} aria-label={summary}><UiBoundary key={`${instanceId}:${retry}`} fallback={fallback}>{failed ? fallback : instance ? <LoadedUi instance={instance} conversationId={conversationId} active={active} enabled={enabled} /> : <p>{summary} · {t('genui.loading')}</p>}</UiBoundary></div>;
}
