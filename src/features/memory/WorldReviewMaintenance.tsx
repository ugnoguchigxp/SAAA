import { reviewApi } from "./reviewApi";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { personalStateSnapshotSchema, type PersonalStateSnapshot } from "./api";

export function WorldReviewMaintenance({
  snapshot,
  setError,
}: {
  snapshot: PersonalStateSnapshot;
  setError: (message: string) => void;
}) {
  const { t } = useTranslation();
  const [review, setReview] = useState(snapshot.maintenance?.retrospective);
  const [busy, setBusy] = useState(false);
  const [candidates, setCandidates] = useState<Awaited<ReturnType<typeof reviewApi.candidates>>>(
    [],
  );
  useEffect(() => {
    setReview(snapshot.maintenance?.retrospective);
    setCandidates([]);
  }, [snapshot]);
  if (!review) return null;
  return (
    <section aria-label={t("memoryPage.review.title")}>
      <label>
        {t("memoryPage.review.title")}
        <select
          value={review.mode}
          disabled={busy}
          onChange={(event) => {
            const mode = event.currentTarget.value as "off" | "preview" | "apply";
            setBusy(true);
            void reviewApi
              .setMode(mode)
              .then((next) => {
                setReview(personalStateSnapshotSchema.parse(next).maintenance?.retrospective);
                setCandidates([]);
                setError("");
              })
              .catch((cause) => setError(String(cause)))
              .finally(() => setBusy(false));
          }}
        >
          <option value="off">{t("memoryPage.review.off")}</option>
          <option value="preview">{t("memoryPage.review.preview")}</option>
          <option value="apply">{t("memoryPage.review.apply")}</option>
        </select>
      </label>
      <p>{t("memoryPage.review.description")}</p>
      <ul>
        {review.stages.map((stage) => (
          <li key={`${stage.stage}:${stage.reason}`}>
            {t(`memoryPage.review.reasons.${stage.reason}`, {
              defaultValue: stage.reason,
            })}
            : {stage.count}
          </li>
        ))}
      </ul>
      <button
        type="button"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          void reviewApi
            .candidates()
            .then(setCandidates)
            .catch((cause) => setError(String(cause)))
            .finally(() => setBusy(false));
        }}
      >
        {t("memoryPage.review.inspect")}
      </button>
      <ul>
        {candidates.map((item) => (
          <li key={item.id}>
            <p>
              {item.scope} ·{" "}
              {t(`memoryPage.review.reasons.${item.reason}`, {
                defaultValue: item.reason,
              })}
            </p>
            <ul>
              {item.sources.map((source) => (
                <li key={`${source.id}:${source.version}`}>
                  {source.id} · v{source.version} · {new Date(source.observedAt).toLocaleString()}
                </li>
              ))}
            </ul>
            {item.candidates.map((candidate, index) => (
              <div key={index}>
                <p>
                  {candidate.kind} · {JSON.stringify(candidate.payload)}
                </p>
                <blockquote>{candidate.quote}</blockquote>
              </div>
            ))}
          </li>
        ))}
      </ul>
    </section>
  );
}
