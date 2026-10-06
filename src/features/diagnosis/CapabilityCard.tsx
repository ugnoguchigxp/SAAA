import { useTranslation } from "react-i18next";
import type {
  DiagnosisCapabilityReport,
  DiagnosisEvidence,
  DiagnosisState,
} from "../../lib/generated/diagnosis";
import { age, sourceKey } from "./diagnosisModel";

const GLYPH: Record<DiagnosisState, string> = {
  ready: "●",
  degraded: "▲",
  unavailable: "✕",
  unverified: "?",
  disabled: "–",
};

export function useAgeText() {
  const { t } = useTranslation();
  return (now: number, at: number | null) => {
    const { unit, count } = age(now, at);
    return t(`chat.diagnosis.age.${unit}`, { count });
  };
}

/** Numeric details are formatted here so the backend never carries wording. */
function useDetailText() {
  const { t } = useTranslation();
  return (evidence: DiagnosisEvidence) =>
    evidence.detail == null
      ? ""
      : !/^\d+(\.\d+)?$/.test(evidence.detail)
        ? evidence.detail
        : t(`chat.diagnosis.detail.${sourceKey(evidence.source)}`, {
            value: evidence.detail,
            defaultValue: evidence.detail,
          });
}

export function CapabilityCard({
  item,
  now,
  busy,
  onRetest,
  onOpenSettings,
}: {
  item: DiagnosisCapabilityReport;
  now: number;
  busy: boolean;
  onRetest: () => void;
  onOpenSettings?: () => void;
}) {
  const { t } = useTranslation();
  const ageText = useAgeText();
  const detailText = useDetailText();
  const id = item.capability;
  const name = t(`chat.diagnosis.capabilities.${id}.name`);
  const deciding = item.evidence.find(
    (evidence) => evidence.outcome !== "pass" && evidence.reason === item.reason && evidence.detail,
  );
  const verified =
    item.verifiedAt == null
      ? t("chat.diagnosis.verified.never")
      : t("chat.diagnosis.verified.at", { age: ageText(now, item.verifiedAt) });
  const canOpenSettings = item.actions.includes("open-settings") && onOpenSettings != null;

  return (
    <li
      className={`dx-card dx-state-${item.state}${item.optional ? " dx-optional" : ""}`}
      data-capability={id}
      data-state={item.state}
    >
      <div className="dx-card-head">
        <div>
          <h3>{name}</h3>
          <p className="dx-muted">{t(`chat.diagnosis.capabilities.${id}.about`)}</p>
        </div>
        <span className="dx-badge">
          <span aria-hidden="true">{GLYPH[item.state]}</span>
          {t(`chat.diagnosis.state.${item.state}`)}
        </span>
      </div>
      <p className="dx-reason">
        {t(`chat.diagnosis.reason.${item.reason}`)}
        {deciding?.detail ? <small> · {detailText(deciding)}</small> : null}
      </p>
      <p className="dx-muted dx-verified">{verified}</p>
      <div className="dx-actions">
        <button
          type="button"
          onClick={onRetest}
          disabled={busy}
          aria-label={`${t("chat.diagnosis.retest")}: ${name}`}
        >
          {t("chat.diagnosis.retest")}
        </button>
        {canOpenSettings ? (
          <button
            type="button"
            onClick={onOpenSettings}
            aria-label={`${t("chat.diagnosis.openSettings")}: ${name}`}
          >
            {t("chat.diagnosis.openSettings")}
          </button>
        ) : null}
      </div>
      {item.evidence.length > 0 ? (
        <details className="dx-evidence">
          <summary>{t("chat.diagnosis.evidence", { count: item.evidence.length })}</summary>
          <ul>
            {item.evidence.map((evidence) => (
              <EvidenceRow
                key={`${evidence.source}|${evidence.route}|${evidence.tier}`}
                evidence={evidence}
                now={now}
              />
            ))}
          </ul>
        </details>
      ) : null}
    </li>
  );
}

function EvidenceRow({ evidence, now }: { evidence: DiagnosisEvidence; now: number }) {
  const { t } = useTranslation();
  const ageText = useAgeText();
  const detailText = useDetailText();
  const label =
    evidence.subject ??
    t(`chat.diagnosis.sources.${sourceKey(evidence.source)}`, { defaultValue: evidence.source });
  return (
    <li className={`dx-evidence-row dx-outcome-${evidence.outcome}`}>
      <span className="dx-evidence-name">{label}</span>
      <span className="dx-chip">{t(`chat.diagnosis.tier.${evidence.tier}`)}</span>
      <span className="dx-evidence-outcome">{t(`chat.diagnosis.outcome.${evidence.outcome}`)}</span>
      <span className="dx-muted dx-evidence-note">
        {[
          evidence.outcome === "pass" ? "" : t(`chat.diagnosis.reason.${evidence.reason}`),
          detailText(evidence),
          evidence.latencyMs != null ? `${evidence.latencyMs} ms` : "",
          ageText(now, evidence.observedAt),
        ]
          .filter(Boolean)
          .join(" · ")}
      </span>
    </li>
  );
}
