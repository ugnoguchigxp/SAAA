import { useEffect, useMemo, useState } from "react";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import { useMeetingSession } from "./useMeetingSession";

export function MeetingPage({ voiceSettings, conversationBusy, onBeforeCapture, onStateChanged }: { voiceSettings: VoiceSettings | null; conversationBusy: boolean; onBeforeCapture: () => Promise<void>; onStateChanged: (state: MeetingState) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [decision, setDecision] = useState<"save" | "discard" | null>(null);
  const meeting = useMeetingSession(voiceSettings, onBeforeCapture, onStateChanged, setError);
  const canStart =
    Boolean(voiceSettings) && !conversationBusy &&
    (meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready");
  const elapsed = useMemo(() => `${Math.floor(meeting.elapsed / 60)}:${String(meeting.elapsed % 60).padStart(2, "0")}`, [meeting.elapsed]);
  const finalEntries = useMemo(() => meeting.transcript.filter((line) => !line.partial), [meeting.transcript]);
  const languages = useMemo(() => [...new Set(finalEntries.map((line) => line.language).filter((language): language is string => Boolean(language)))], [finalEntries]);
  useEffect(() => {
    if (meeting.snapshot.state !== "completed") setDecision(null);
  }, [meeting.snapshot.state]);
  return <section className="meeting-page">
    <header className="topbar"><div><p className="eyebrow">EXPLICIT MEETING MODE</p><h1>Meeting</h1></div><span className={meeting.snapshot.state === "active" ? "badge warning" : "badge local"}>{meeting.snapshot.state}</span></header>
    <div className="route-banner"><strong>Microphone · {meeting.health}</strong><span>System audio: Unavailable in this build</span><span>STT: Voice設定で選択したASR</span></div>
    <p className="meeting-meta">Elapsed {elapsed} · Persistence: Discard until you explicitly save after stopping.</p>
    {error && <p className="error-banner" role="alert">{error}</p>}
    {(meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready") && <button className="send-button" disabled={!canStart || meeting.working} onClick={() => void meeting.start()}>{meeting.working ? "Starting…" : "Start meeting"}</button>}
    {(meeting.snapshot.state === "active" || meeting.snapshot.state === "paused") && <div className="meeting-actions">{meeting.snapshot.state === "active" ? <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.pause()}>Pause</button> : <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.resume()}>Resume</button>}<button className="stop-button" disabled={meeting.working} onClick={() => void meeting.stop()}>{meeting.working ? "Updating…" : "Stop meeting"}</button></div>}
    {meeting.snapshot.state === "completed" && decision === null && <div className="meeting-actions"><button className="send-button" disabled={meeting.working || finalEntries.length === 0} onClick={() => setDecision("save")}>Review before save</button><button className="stop-button" disabled={meeting.working} onClick={() => setDecision("discard")}>Review discard</button></div>}
    {meeting.snapshot.state === "completed" && decision === "save" && <section className="meeting-decision" aria-label="Save transcript confirmation"><h2>Save transcript?</h2><dl><div><dt>Target</dt><dd>SAAA local SQLite database</dd></div><div><dt>Entries</dt><dd>{finalEntries.length} final transcript entries</dd></div><div><dt>Languages</dt><dd>{languages.length ? languages.join(", ") : "Not detected"}</dd></div><div><dt>Audio</dt><dd>Raw microphone audio is deleted and is not saved.</dd></div></dl><div className="meeting-actions"><button className="send-button" disabled={meeting.working || finalEntries.length === 0} onClick={() => void meeting.save()}>Confirm save</button><button className="secondary-button" disabled={meeting.working} onClick={() => setDecision(null)}>Back</button></div></section>}
    {meeting.snapshot.state === "completed" && decision === "discard" && <section className="meeting-decision danger" aria-label="Discard transcript confirmation"><h2>Discard this transcript?</h2><p>{finalEntries.length} transcript entries and their language metadata will not be saved. This cannot be undone.</p><div className="meeting-actions"><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>Confirm discard</button><button className="secondary-button" disabled={meeting.working} onClick={() => setDecision(null)}>Back</button></div></section>}
    {meeting.snapshot.state === "failed" && meeting.snapshot.sessionId && <div className="meeting-actions"><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>Discard failed session</button></div>}
    <div className="message-area meeting-transcript">{meeting.transcript.length ? meeting.transcript.map((line) => <article className="message transcript" key={line.sequence}><span className="message-role">{line.lane} · {line.sequence + 1} · {line.partial ? "partial" : "final"}{line.language ? ` · ${line.language}` : ""}</span><p>{line.text}</p></article>) : <div className="empty-state"><h2>Explicit capture only.</h2><p>Start controls the microphone. Audio is sent to the ASR selected in Voice settings in short segments and is never saved unless you choose Save after Stop.</p></div>}</div>
  </section>;
}
