import { useMemo, useState } from "react";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import { useMeetingSession } from "./useMeetingSession";

export function MeetingPage({ voiceSettings, chatVoiceBusy, onStateChanged }: { voiceSettings: VoiceSettings | null; chatVoiceBusy: boolean; onStateChanged: (state: MeetingState) => void }) {
  const [error, setError] = useState<string | null>(null);
  const meeting = useMeetingSession(voiceSettings, onStateChanged, setError);
  const canStart =
    Boolean(voiceSettings?.sttModel.trim()) && !chatVoiceBusy &&
    (meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready");
  const elapsed = useMemo(() => `${Math.floor(meeting.elapsed / 60)}:${String(meeting.elapsed % 60).padStart(2, "0")}`, [meeting.elapsed]);
  return <section className="meeting-page">
    <header className="topbar"><div><p className="eyebrow">EXPLICIT MEETING MODE</p><h1>Meeting</h1></div><span className={meeting.snapshot.state === "active" ? "badge warning" : "badge local"}>{meeting.snapshot.state}</span></header>
    <div className="route-banner"><strong>Microphone · {meeting.health}</strong><span>System audio: Unavailable in this build</span><span>STT: local-whisper / {voiceSettings?.sttModel.split("/").pop() || "no model"}</span></div>
    <p className="meeting-meta">Elapsed {elapsed} · Persistence: Discard until you explicitly save after stopping.</p>
    {error && <p className="error-banner" role="alert">{error}</p>}
    {(meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready") && <button className="send-button" disabled={!canStart || meeting.working} onClick={() => void meeting.start()}>{meeting.working ? "Starting…" : "Start meeting"}</button>}
    {(meeting.snapshot.state === "active" || meeting.snapshot.state === "paused") && <div className="meeting-actions">{meeting.snapshot.state === "active" ? <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.pause()}>Pause</button> : <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.resume()}>Resume</button>}<button className="stop-button" disabled={meeting.working} onClick={() => void meeting.stop()}>{meeting.working ? "Updating…" : "Stop meeting"}</button></div>}
    {meeting.snapshot.state === "completed" && <div className="meeting-actions"><button className="send-button" disabled={meeting.working} onClick={() => void meeting.save()}>Save {meeting.transcript.length} transcript entries</button><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>Discard</button></div>}
    {meeting.snapshot.state === "failed" && meeting.snapshot.sessionId && <div className="meeting-actions"><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>Discard failed session</button></div>}
    <div className="message-area meeting-transcript">{meeting.transcript.length ? meeting.transcript.map((line) => <article className="message transcript" key={line.sequence}><span className="message-role">microphone · {line.sequence + 1} · {line.partial ? "partial" : "final"}</span><p>{line.text}</p></article>) : <div className="empty-state"><h2>Explicit capture only.</h2><p>Start controls the microphone. Audio is transcribed locally in short segments and is never saved unless you choose Save after Stop.</p></div>}</div>
  </section>;
}
