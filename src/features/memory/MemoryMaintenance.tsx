import { MaintenanceNotices } from "./MaintenanceNotices";
import { WorldReviewMaintenance } from "./WorldReviewMaintenance";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PersonalStateSnapshot } from "./api";

/** Maintenance controls and World questions share the existing conversation action. */
export function MemoryMaintenance({
  view,
  onViewChange,
  snapshot,
  loading,
  onSetEnabled,
  onAsk,
  setError,
}: {
  view: "state" | "world";
  onViewChange: (view: "state" | "world") => void;
  snapshot: PersonalStateSnapshot | null;
  loading: boolean;
  onSetEnabled: (enabled: boolean) => Promise<void>;
  onAsk?: (question: string) => Promise<void>;
  setError: (message: string) => void;
}) {
  const { t } = useTranslation();
  const [question, setQuestion] = useState("");
  const [asking, setAsking] = useState(false);
  return (
    <>
      <div role="group" aria-label={t("memoryPage.viewLabel")}>
        <button
          type="button"
          aria-pressed={view === "state"}
          onClick={() => {
            onViewChange("state");
          }}
        >
          {t("memoryPage.stateView")}
        </button>
        <button
          type="button"
          aria-pressed={view === "world"}
          onClick={() => {
            onViewChange("world");
          }}
        >
          {t("memoryPage.worldView")}
        </button>
      </div>
      {view === "world" && onAsk ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (!question.trim() || asking) return;
            setAsking(true);
            void onAsk(question.trim())
              .then(() => {
                setQuestion("");
                setError("");
              })
              .catch((cause) => setError(String(cause)))
              .finally(() => setAsking(false));
          }}
        >
          <label>
            {t("memoryPage.askLabel")}
            <input
              value={question}
              maxLength={512}
              onChange={(event) => setQuestion(event.currentTarget.value)}
              placeholder={t("memoryPage.askPlaceholder")}
            />
          </label>
          <button type="submit" disabled={asking || !question.trim()}>
            {t("memoryPage.askSubmit")}
          </button>
        </form>
      ) : null}
      {snapshot ? (
        <label>
          <input
            type="checkbox"
            checked={snapshot.enabled}
            disabled={loading || snapshot.enabledOverride}
            onChange={(event) => {
              void onSetEnabled(event.currentTarget.checked);
            }}
          />
          {t("memoryPage.enableMaintenance")}
        </label>
      ) : null}
      <MaintenanceNotices snapshot={snapshot} />
      {view === "world" && snapshot ? (
        <WorldReviewMaintenance snapshot={snapshot} setError={setError} />
      ) : null}
      {snapshot?.maintenance ? (
        <p role="status">
          {t("memoryPage.maintenanceStatus", {
            reason: t(
              `memoryPage.maintenanceReasons.${snapshot.maintenance.reason}`,
            ),
          })}
        </p>
      ) : null}
      {snapshot?.maintenance?.work ? (
        <p role="status">
          {t("memoryPage.workStatus", snapshot.maintenance.work)}
        </p>
      ) : null}
    </>
  );
}
