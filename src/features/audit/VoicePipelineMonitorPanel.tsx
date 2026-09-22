import { useTranslation } from "react-i18next";
import { auditTimestampIso, formatAuditTimestamp } from "./auditTimestamp";
import type { VoicePipelineSnapshot } from "./voicePipelineMonitor";
import {
  harnessFailureHintKeys,
  harnessProgressHintKeys,
  isHarnessProgressCode,
} from "../../lib/harnessFailureDiagnostics";
import "./VoicePipelineMonitorPanel.css";

const failureHintKeys: Partial<Record<string, string>> = {
  ...harnessFailureHintKeys,
  "context-scope-changed": "chat.contextRecovery.context-scope-changed",
  "required-context-overflow": "chat.contextRecovery.required-context-overflow",
  "required-context-unavailable": "chat.contextRecovery.required-context-unavailable",
};

const lfmDiagnosis: Record<string, string> = {
  "lfm-failed": "LFMの会話応対が失敗しました。LFMカードの具体的な失敗コードを確認してください。",
  "lfm-running": "LFMが発言を受け取り、定型応答と思考依頼の要否を判断しています。",
  "lfm-responded": "LFMが受付を完了しました。この発言はQwenへの思考依頼を必要としません。",
  "lfm-reasoning-requested":
    "LFMがQwenへ思考を依頼しました。追加の発言は引き続きLFMが受け取ります。",
};

export function VoicePipelineMonitor({
  snapshot,
  locale,
}: {
  snapshot: VoicePipelineSnapshot;
  locale: string;
}) {
  const { t } = useTranslation();
  const failed = snapshot.stages.some(
    (stage) => stage.state === "failure" || stage.state === "blocked",
  );

  return (
    <section className="pipeline-monitor" aria-labelledby="pipeline-monitor-title">
      <div className="pipeline-monitor-heading">
        <div>
          <p>{t("audit.monitor.eyebrow")}</p>
          <h3 id="pipeline-monitor-title">{t("audit.monitor.title")}</h3>
        </div>
        <span className="pipeline-monitor-live">{t("audit.monitor.live")}</span>
      </div>

      {snapshot.anchor && (
        <p className="pipeline-request-time">
          {t("audit.monitor.requestTime")}{" "}
          {formatAuditTimestamp(snapshot.anchor.occurredAt, locale)}
          {" — "}
          {t("audit.monitor.savedResult")}
          {snapshot.runId && <small> ({snapshot.runId})</small>}
        </p>
      )}

      <p className={`pipeline-diagnosis pipeline-diagnosis-${failed ? "failure" : "normal"}`}>
        {lfmDiagnosis[snapshot.diagnosis] ?? t(`audit.monitor.diagnosis.${snapshot.diagnosis}`)}
      </p>

      <div className="pipeline-stages">
        {snapshot.stages.map((stage, index) => (
          <div className="pipeline-stage-wrap" key={stage.key}>
            <article className={`pipeline-stage pipeline-stage-${stage.state}`}>
              <div className="pipeline-stage-title">
                <strong>
                  {stage.key === "lfm"
                    ? "LFM · 会話応対"
                    : stage.key === "qwen"
                      ? "Qwen · 推論・実行"
                      : t(`audit.monitor.stages.${stage.key}`)}
                </strong>
                <span>{t(`audit.monitor.states.${stage.state}`)}</span>
              </div>
              <p>{stage.failureCode ?? stage.event?.eventName ?? t("audit.monitor.noEvent")}</p>
              {typeof stage.event?.attributes.reasonCode === "string" &&
                isHarnessProgressCode(stage.event.attributes.reasonCode) && (
                  <small>{t(harnessProgressHintKeys[stage.event.attributes.reasonCode])}</small>
                )}
              {stage.failureCode && failureHintKeys[stage.failureCode] ? (
                <small>{t(failureHintKeys[stage.failureCode]!)}</small>
              ) : null}
            </article>
            {index < snapshot.stages.length - 1 ? (
              <span className="pipeline-arrow" aria-hidden="true">
                →
              </span>
            ) : null}
          </div>
        ))}
      </div>

      {snapshot.relatedEvents.length > 0 ? (
        <div className="pipeline-recent-events">
          <span>{t("audit.monitor.relatedEvents")}</span>
          <ol>
            {snapshot.relatedEvents.slice(0, 5).map((event) => (
              <li key={event.id}>
                <time dateTime={auditTimestampIso(event.occurredAt)}>
                  {formatAuditTimestamp(event.occurredAt, locale)}
                </time>
                <strong>{event.eventName}</strong>
                <span>{event.failureCode ?? event.outcome ?? event.phase}</span>
              </li>
            ))}
          </ol>
        </div>
      ) : null}
    </section>
  );
}
