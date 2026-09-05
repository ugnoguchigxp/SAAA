import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import { localizeMeetingLane, localizeStatus, localizeUiMessage } from "../../i18n/presentation";
import { useMeetingSession } from "./useMeetingSession";
export function MeetingPage({ voiceSettings, conversationBusy, onBeforeCapture, onStateChanged }: { voiceSettings: VoiceSettings | null; conversationBusy: boolean; onBeforeCapture: () => Promise<void>; onStateChanged: (state: MeetingState) => void }) {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);
  const [decision, setDecision] = useState<"save" | "discard" | null>(null);
  const meeting = useMeetingSession(voiceSettings, onBeforeCapture, onStateChanged, setError);
  const canStart =
    Boolean(voiceSettings) && !conversationBusy &&
    (meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready");
  const elapsed = useMemo(() => `${Math.floor(meeting.elapsed / 60)}:${String(meeting.elapsed % 60).padStart(2, "0")}`, [meeting.elapsed]);
  const languages = useMemo(() => [...new Set(meeting.transcript.map((line) => line.language).filter((language): language is string => Boolean(language)))], [meeting.transcript]);
  useEffect(() => {
    if (meeting.snapshot.state !== "completed") setDecision(null);
  }, [meeting.snapshot.state]);
  return <section className="meeting-page">
    <header className="topbar"><div><p className="eyebrow">{t("meeting.eyebrow")}</p><h1>{t("meeting.title")}</h1></div><span className={meeting.snapshot.state === "active" ? "badge warning" : "badge local"}>{t(`meeting.states.${meeting.snapshot.state}`, { defaultValue: meeting.snapshot.state })}</span></header>
    <div className="route-banner"><strong>{t("meeting.microphone")} · {localizeStatus(t, meeting.health)}</strong><span>{t("meeting.systemAudioUnavailable")}</span><span>{t("meeting.sttRoute")}</span><span>{t(`meeting.transcriptionScopes.${meeting.snapshot.transcriptionScope}`)}</span></div>
    <p className="meeting-meta">{t("meeting.elapsed", { time: elapsed })}</p>
    {error && <p className="error-banner" role="alert">{localizeUiMessage(t, error, "meeting")}</p>}
    {(meeting.snapshot.state === "idle" || meeting.snapshot.state === "ready") && <button className="send-button" disabled={!canStart || meeting.working} onClick={() => void meeting.start()}>{meeting.working ? t("meeting.starting") : t("meeting.start")}</button>}
    {(meeting.snapshot.state === "active" || meeting.snapshot.state === "paused") && <div className="meeting-actions">{meeting.snapshot.state === "active" ? <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.pause()}>{t("meeting.pause")}</button> : <button className="secondary-button" disabled={meeting.working} onClick={() => void meeting.resume()}>{t("meeting.resume")}</button>}<button className="stop-button" disabled={meeting.working} onClick={() => void meeting.stop()}>{meeting.working ? t("meeting.updating") : t("meeting.stop")}</button></div>}
    {meeting.snapshot.state === "completed" && decision === null && <div className="meeting-actions"><button className="send-button" disabled={meeting.working || meeting.transcript.length === 0} onClick={() => setDecision("save")}>{t("meeting.reviewSave")}</button><button className="stop-button" disabled={meeting.working} onClick={() => setDecision("discard")}>{t("meeting.reviewDiscard")}</button></div>}
    {meeting.snapshot.state === "completed" && decision === "save" && <section className="meeting-decision" aria-label={t("meeting.saveAria")}><h2>{t("meeting.saveTitle")}</h2><dl><div><dt>{t("meeting.target")}</dt><dd>{t("meeting.targetValue")}</dd></div><div><dt>{t("meeting.entries")}</dt><dd>{t("meeting.entriesValue", { count: meeting.transcript.length })}</dd></div><div><dt>{t("meeting.languages")}</dt><dd>{languages.length ? languages.map((language) => t(`asrLanguages.${language}`, { defaultValue: t("meeting.notDetected") })).join(", ") : t("meeting.notDetected")}</dd></div><div><dt>{t("meeting.audio")}</dt><dd>{t("meeting.audioValue")}</dd></div></dl><div className="meeting-actions"><button className="send-button" disabled={meeting.working || meeting.transcript.length === 0} onClick={() => void meeting.save()}>{t("meeting.confirmSave")}</button><button className="secondary-button" disabled={meeting.working} onClick={() => setDecision(null)}>{t("common.back")}</button></div></section>}
    {meeting.snapshot.state === "completed" && decision === "discard" && <section className="meeting-decision danger" aria-label={t("meeting.discardAria")}><h2>{t("meeting.discardTitle")}</h2><p>{t("meeting.discardDescription", { count: meeting.transcript.length })}</p><div className="meeting-actions"><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>{t("meeting.confirmDiscard")}</button><button className="secondary-button" disabled={meeting.working} onClick={() => setDecision(null)}>{t("common.back")}</button></div></section>}
    {meeting.snapshot.state === "failed" && meeting.snapshot.sessionId && <div className="meeting-actions"><button className="stop-button" disabled={meeting.working} onClick={() => void meeting.discard()}>{t("meeting.discardFailed")}</button></div>}
    <div className="message-area meeting-transcript">{meeting.transcript.length ? meeting.transcript.map((line) => <article className="message transcript" key={line.sequence}><span className="message-role">{localizeMeetingLane(t, line.lane)} · {line.sequence + 1} · {t("meeting.final")}{line.language ? ` · ${t(`asrLanguages.${line.language}`, { defaultValue: t("meeting.notDetected") })}` : ""}</span><p>{line.text}</p></article>) : <div className="empty-state"><h2>{t("meeting.emptyTitle")}</h2><p>{t("meeting.emptyDescription")}</p></div>}</div>
  </section>;
}
