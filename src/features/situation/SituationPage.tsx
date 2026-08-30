import { useCallback, useEffect, useRef, useState } from "react";
import type { SituationLedgerEntry, SituationSnapshot } from "../../lib/contracts";
import {
  clearSituationHistory,
  getSituationSnapshot,
  setSituationMonitoring,
  submitSituationFeedback,
} from "../../lib/runtime";
import { SituationReview } from "./review/SituationReview";

const attentionLabels = {
  IGNORE: "Stay silent",
  OBSERVE: "Would observe",
  SUGGEST: "Would suggest",
  RESPOND: "Would respond",
} as const;

const evidenceLabels: Record<string, string> = {
  "explicit-user-input": "Explicit user input",
  "model-run-active": "Conversation generation is active",
  "agent-run-active": "Coding agent is active",
  "saaa-capture-active": "Continuous voice capture is active",
  "saaa-transcription-active": "Local transcription is active",
  "sensitive-application": "Sensitive application category",
  "communication-app": "Communication application category",
  "calendar-meeting-likely": "Calendar indicates a likely meeting",
  "external-microphone-active": "External microphone activity",
  "coding-app": "Coding application category",
  "writing-app": "Writing application category",
  "calendar-busy": "Calendar indicates busy time",
  "media-app": "Media application category",
  "external-media-active": "External media activity",
  "foreground-app-available": "Foreground application is available",
};

export function SituationPage({ onSettingsChanged }: { onSettingsChanged: () => Promise<void> }) {
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
    const reasonCode = verdict === "inaccurate" ? window.prompt("Reason code: wrong-scene, stale-signal, unstable-transition, unwanted-suggestion, missed-meeting-candidate, insufficient-evidence") : null;
    if (verdict === "inaccurate" && !reasonCode) return;
    const correctedScene = verdict === "inaccurate" ? window.prompt("Corrected scene (optional): CONVERSATION, MEETING, CODING, WRITING, MEDIA, FOCUS, SOLO, UNKNOWN") : null;
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
    if (!window.confirm("Situation ledger, feedback, quality windows, and calibration runs will be deleted. Active profiles, conversations, and Settings are kept.")) return;
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

  if (!snapshot) return <section className="situation-page"><div className="situation-loading">Situation Runtimeを読み込んでいます…</div>{error && <p role="alert">{error}</p>}</section>;

  const latest = snapshot.history[0];
  return <section className="situation-page">
    <header className="situation-header"><div><p className="eyebrow">SITUATION SHADOW MODE</p><h1>Situation</h1><p>ローカルHard Signalから、現在の候補と介入案を観測します。</p></div><button className={snapshot.monitoringEnabled ? "stop-button" : "save-button situation-toggle"} onClick={() => void toggleMonitoring()} disabled={working}>{working ? "Updating…" : snapshot.monitoringEnabled ? "Pause observation" : "Enable observation"}</button></header>
    <div className="situation-safety"><strong>No automatic actions</strong><span>Execution NONE · Presentation SILENT · Model/TTS/Notification/Application actions disabled</span></div>
    <div className="situation-content"><div className="feedback-row" role="tablist"><button className={view === "overview" ? "selected" : ""} onClick={() => setView("overview")}>Overview</button><button className={view === "review" ? "selected" : ""} onClick={() => setView("review")}>Review</button></div>{view === "review" ? <SituationReview /> : <>
      <section className="situation-hero"><div><span className="situation-kicker">CURRENT STABLE SCENE</span><strong>{snapshot.state.scene}</strong><small>{snapshot.state.confidence}% confidence · {snapshot.monitoringEnabled ? snapshot.monitoringActive ? "observation active" : "observation starting" : "observation paused"} · rule {snapshot.state.ruleVersion}</small></div><div className="shadow-decision"><span>Shadow decision</span><strong>{attentionLabels[snapshot.decision.proposedAttention]}</strong><small>{snapshot.decision.reasonCodes.join(" · ")}</small></div></section>

      <div className="situation-grid">
        <section className="situation-card"><div className="section-heading"><h2>Evidence</h2><span>{snapshot.state.userAttention} attention</span></div>{snapshot.state.evidence.length > 0 ? <ul className="evidence-list">{snapshot.state.evidence.map((item) => <li key={item.code}><span>{evidenceLabels[item.code] ?? item.code}</span><strong>+{item.weight}</strong></li>)}</ul> : <p className="settings-help">判定に十分なfresh signalがありません。Safe defaultを維持します。</p>}</section>
        <section className="situation-card"><div className="section-heading"><h2>Signal health</h2><span>Sequence {snapshot.signals.sequence}</span></div><div className="signal-health-grid"><Signal label="Foreground" value={`${snapshot.signals.foreground.category} · ${snapshot.signals.foreground.health}`} /><Signal label="Input activity" value={`${activityLabel(snapshot.signals.inputActivity.state)} · ${snapshot.signals.inputActivity.health}`} /><Signal label="Conversation" value={snapshot.signals.conversation.state} /><Signal label="Microphone" value={`${snapshot.signals.microphone.state} · ${snapshot.signals.microphone.health}`} /><Signal label="Audio" value={`${snapshot.signals.audio.state} · ${snapshot.signals.audio.health}`} /><Signal label="Calendar" value={`${snapshot.signals.calendar.state} · ${snapshot.signals.calendar.health}`} /></div><p className="settings-help">Raw application identity、window title、Calendar details、audio content、exact input idle timeは保存しません。</p></section>
      </div>

      <section className="situation-card timeline-card"><div className="section-heading"><div><h2>Evaluation ledger</h2><p>{snapshot.evaluation.totalEntries} bounded entries · {snapshot.evaluation.accurate} accurate · {snapshot.evaluation.inaccurate} inaccurate · {snapshot.evaluation.unsure} unsure</p></div><button className="text-button danger" onClick={() => void clearHistory()} disabled={working || snapshot.history.length === 0}>Clear history</button></div>{snapshot.history.length === 0 ? <p className="settings-help">Monitoringを有効にすると、transition、decision変更、bounded heartbeatがここへ保存されます。</p> : <div className="situation-timeline">{snapshot.history.map((entry) => <article key={entry.id}><div className="timeline-marker" /><div className="timeline-body"><div className="timeline-title"><strong>{entry.state.scene}</strong><span>{attentionLabels[entry.decision.proposedAttention]} · {entry.entryKind}</span><time>{formatTime(entry.observedAt)}</time></div><p>{entry.state.evidence.map((item) => evidenceLabels[item.code] ?? item.code).join(" · ") || "Safe default"}</p><div className="feedback-row" role="group" aria-label={`Evaluate ${entry.state.scene}`}><button disabled={working} className={entry.feedback?.verdict === "accurate" ? "selected" : ""} onClick={() => void feedback(entry, "accurate")}>Accurate</button><button disabled={working} className={entry.feedback?.verdict === "inaccurate" ? "selected" : ""} onClick={() => void feedback(entry, "inaccurate")}>Inaccurate</button><button disabled={working} className={entry.feedback?.verdict === "unsure" ? "selected" : ""} onClick={() => void feedback(entry, "unsure")}>Unsure</button></div></div></article>)}</div>}</section>
      {latest && <p className="situation-footnote">Latest persisted entry: {latest.id} · policy {latest.decision.policyVersion}</p>}
    </>}</div>
    {snapshot.lastFailure && <p className="error-banner" role="alert">{snapshot.lastFailure.message} {snapshot.lastFailure.recovery}</p>}
    {error && <p className="error-banner" role="alert">{error}</p>}
  </section>;
}

function Signal({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function activityLabel(value: SituationSnapshot["signals"]["inputActivity"]["state"]): string {
  return ({ active: "Active", recent: "Recent", idle: "Idle", unknown: "Unknown" })[value];
}

function formatTime(value: string): string {
  const milliseconds = Number(value);
  return Number.isFinite(milliseconds) ? new Date(milliseconds).toLocaleString() : value;
}

function toMessage(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }
