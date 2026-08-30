import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { SituationLedgerEntry, SituationSnapshot } from "../../lib/contracts";
import {
  clearSituationHistory,
  getSituationSnapshot,
  setSituationMonitoring,
  submitSituationFeedback,
} from "../../lib/runtime";
import { SituationReview } from "./review/SituationReview";
import {
  formatRegionalDateTime,
  localizeForegroundCategory,
  localizeSituationAttention,
  localizeSituationEntryKind,
  localizeSituationReason,
  localizeSituationScene,
  localizeStatus,
  localizeUiMessage,
} from "../../i18n/presentation";

const attentionKeys = { IGNORE: "IGNORE", OBSERVE: "OBSERVE", SUGGEST: "SUGGEST", RESPOND: "RESPOND" } as const;

const evidenceKeys: Record<string, string> = {
  "explicit-user-input": "explicitUserInput",
  "model-run-active": "modelRunActive",
  "agent-run-active": "agentRunActive",
  "saaa-capture-active": "saaaCaptureActive",
  "saaa-transcription-active": "saaaTranscriptionActive",
  "sensitive-application": "sensitiveApplication",
  "communication-app": "communicationApp",
  "calendar-meeting-likely": "calendarMeetingLikely",
  "external-microphone-active": "externalMicrophoneActive",
  "coding-app": "codingApp",
  "writing-app": "writingApp",
  "calendar-busy": "calendarBusy",
  "media-app": "mediaApp",
  "external-media-active": "externalMediaActive",
  "foreground-app-available": "foregroundAppAvailable",
};

export function SituationPage({ onSettingsChanged, timeZone }: { onSettingsChanged: () => Promise<void>; timeZone: string }) {
  const { t, i18n } = useTranslation();
  const [view, setView] = useState<"overview" | "review">("overview");
  const [snapshot, setSnapshot] = useState<SituationSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const workingRef = useRef(false);
  const requestRevision = useRef(0);

  const refresh = useCallback(async () => {
    if (workingRef.current) return;
    const revision = ++requestRevision.current;
    try {
      const next = await getSituationSnapshot();
      if (revision === requestRevision.current && !workingRef.current) {
        setSnapshot(next);
        setError(null);
      }
    } catch (cause) {
      if (revision === requestRevision.current) setError(toMessage(cause));
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!snapshot?.monitoringEnabled) return;
    const interval = window.setInterval(() => { void refresh(); }, 2_000);
    return () => window.clearInterval(interval);
  }, [refresh, snapshot?.monitoringEnabled]);

  async function toggleMonitoring() {
    if (!snapshot || !beginMutation()) return;
    const revision = requestRevision.current;
    try {
      const next = await setSituationMonitoring(!snapshot.monitoringEnabled);
      if (revision === requestRevision.current) setSnapshot(next);
      await onSettingsChanged();
      setError(null);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endMutation();
    }
  }

  async function feedback(entry: SituationLedgerEntry, verdict: "accurate" | "inaccurate" | "unsure") {
    const reasonCode = verdict === "inaccurate" ? window.prompt(t("situation.feedbackReasonPrompt")) : null;
    if (verdict === "inaccurate" && !reasonCode) return;
    const correctedScene = verdict === "inaccurate" ? window.prompt(t("situation.correctedScenePrompt")) : null;
    if (!beginMutation()) return;
    const revision = requestRevision.current;
    try {
      const next = await submitSituationFeedback({
        ledgerId: entry.id,
        verdict,
        impact: "none",
        correctedScene: correctedScene || null,
        reasonCode,
      });
      if (revision === requestRevision.current) setSnapshot(next);
      setError(null);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endMutation();
    }
  }

  async function clearHistory() {
    if (!window.confirm(t("situation.confirmClear"))) return;
    if (!beginMutation()) return;
    const revision = requestRevision.current;
    try {
      const next = await clearSituationHistory();
      if (revision === requestRevision.current) setSnapshot(next);
      setError(null);
    } catch (cause) {
      setError(toMessage(cause));
    } finally {
      endMutation();
    }
  }

  function beginMutation(): boolean {
    if (workingRef.current) return false;
    workingRef.current = true;
    requestRevision.current += 1;
    setWorking(true);
    return true;
  }

  function endMutation() {
    workingRef.current = false;
    setWorking(false);
  }

  if (!snapshot) return <section className="situation-page"><div className="situation-loading">{t("situation.loading")}</div>{error && <p role="alert">{localizeUiMessage(t, error, "situation")}</p>}</section>;

  const latest = snapshot.history[0];
  return <section className="situation-page">
    <header className="situation-header"><div><p className="eyebrow">{t("situation.eyebrow")}</p><h1>{t("situation.title")}</h1><p>{t("situation.description")}</p></div><button className={snapshot.monitoringEnabled ? "stop-button" : "save-button situation-toggle"} onClick={() => void toggleMonitoring()} disabled={working}>{working ? t("situation.updating") : snapshot.monitoringEnabled ? t("situation.pauseObservation") : t("situation.enableObservation")}</button></header>
    <div className="situation-safety"><strong>{t("situation.noAutomaticActions")}</strong><span>{t("situation.safetyDetail")}</span></div>
    <div className="situation-content"><div className="feedback-row" role="tablist"><button className={view === "overview" ? "selected" : ""} onClick={() => setView("overview")}>{t("situation.overview")}</button><button className={view === "review" ? "selected" : ""} onClick={() => setView("review")}>{t("situation.review")}</button></div>{view === "review" ? <SituationReview /> : <>
      <section className="situation-hero"><div><span className="situation-kicker">{t("situation.currentScene")}</span><strong>{localizeSituationScene(t, snapshot.state.scene)}</strong><small>{t("situation.confidence", { confidence: snapshot.state.confidence })} · {snapshot.monitoringEnabled ? snapshot.monitoringActive ? t("situation.observationActive") : t("situation.observationStarting") : t("situation.observationPaused")} · {t("situation.rule", { version: snapshot.state.ruleVersion })}</small></div><div className="shadow-decision"><span>{t("situation.shadowDecision")}</span><strong>{t(`situation.attention.${attentionKeys[snapshot.decision.proposedAttention]}`)}</strong><small>{snapshot.decision.reasonCodes.map((reason) => localizeSituationReason(t, reason)).join(" · ")}</small></div></section>

      <div className="situation-grid">
        <section className="situation-card"><div className="section-heading"><h2>{t("situation.evidenceTitle")}</h2><span>{t("situation.attentionValue", { value: localizeSituationAttention(t, snapshot.state.userAttention) })}</span></div>{snapshot.state.evidence.length > 0 ? <ul className="evidence-list">{snapshot.state.evidence.map((item) => <li key={item.code}><span>{evidenceKeys[item.code] ? t(`situation.evidence.${evidenceKeys[item.code]}`) : t("situation.evidenceUnknown")}</span><strong>+{item.weight}</strong></li>)}</ul> : <p className="settings-help">{t("situation.noEvidence")}</p>}</section>
        <section className="situation-card"><div className="section-heading"><h2>{t("situation.signalHealth")}</h2><span>{t("situation.sequence", { value: snapshot.signals.sequence })}</span></div><div className="signal-health-grid"><Signal label={t("situation.foreground")} value={`${localizeForegroundCategory(t, snapshot.signals.foreground.category)} · ${localizeStatus(t, snapshot.signals.foreground.health)}`} /><Signal label={t("situation.inputActivity")} value={`${localizeStatus(t, snapshot.signals.inputActivity.state)} · ${localizeStatus(t, snapshot.signals.inputActivity.health)}`} /><Signal label={t("situation.conversation")} value={localizeStatus(t, snapshot.signals.conversation.state)} /><Signal label={t("situation.microphone")} value={`${localizeStatus(t, snapshot.signals.microphone.state)} · ${localizeStatus(t, snapshot.signals.microphone.health)}`} /><Signal label={t("situation.audio")} value={`${localizeStatus(t, snapshot.signals.audio.state)} · ${localizeStatus(t, snapshot.signals.audio.health)}`} /><Signal label={t("situation.calendar")} value={`${localizeStatus(t, snapshot.signals.calendar.state)} · ${localizeStatus(t, snapshot.signals.calendar.health)}`} /></div><p className="settings-help">{t("situation.privacy")}</p></section>
      </div>

      <section className="situation-card timeline-card"><div className="section-heading"><div><h2>{t("situation.ledger")}</h2><p>{t("situation.ledgerSummary", { total: snapshot.evaluation.totalEntries, accurate: snapshot.evaluation.accurate, inaccurate: snapshot.evaluation.inaccurate, unsure: snapshot.evaluation.unsure })}</p></div><button className="text-button danger" onClick={() => void clearHistory()} disabled={working || snapshot.history.length === 0}>{t("situation.clearHistory")}</button></div>{snapshot.history.length === 0 ? <p className="settings-help">{t("situation.emptyHistory")}</p> : <div className="situation-timeline">{snapshot.history.map((entry) => <article key={entry.id}><div className="timeline-marker" /><div className="timeline-body"><div className="timeline-title"><strong>{localizeSituationScene(t, entry.state.scene)}</strong><span>{t(`situation.attention.${attentionKeys[entry.decision.proposedAttention]}`)} · {localizeSituationEntryKind(t, entry.entryKind)}</span><time>{formatRegionalDateTime(entry.observedAt, i18n.resolvedLanguage, timeZone)}</time></div><p>{entry.state.evidence.map((item) => evidenceKeys[item.code] ? t(`situation.evidence.${evidenceKeys[item.code]}`) : t("situation.evidenceUnknown")).join(" · ") || t("situation.safeDefault")}</p><div className="feedback-row" role="group" aria-label={t("situation.evaluateAria", { scene: localizeSituationScene(t, entry.state.scene) })}><button disabled={working} className={entry.feedback?.verdict === "accurate" ? "selected" : ""} onClick={() => void feedback(entry, "accurate")}>{t("situation.accurate")}</button><button disabled={working} className={entry.feedback?.verdict === "inaccurate" ? "selected" : ""} onClick={() => void feedback(entry, "inaccurate")}>{t("situation.inaccurate")}</button><button disabled={working} className={entry.feedback?.verdict === "unsure" ? "selected" : ""} onClick={() => void feedback(entry, "unsure")}>{t("situation.unsure")}</button></div></div></article>)}</div>}</section>
      {latest && <p className="situation-footnote">{t("situation.latestEntry", { id: latest.id, policy: latest.decision.policyVersion })}</p>}
    </>}</div>
    {snapshot.lastFailure && <p className="error-banner" role="alert">{localizeUiMessage(t, snapshot.lastFailure.message, "situation")}</p>}
    {error && <p className="error-banner" role="alert">{localizeUiMessage(t, error, "situation")}</p>}
  </section>;
}

function Signal({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function toMessage(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }
