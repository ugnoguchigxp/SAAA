import { useTranslation } from "react-i18next";
import type { PersonalStateSnapshot } from "./api";
export function MaintenanceNotices({
  snapshot,
}: {
  snapshot: PersonalStateSnapshot | null;
}) {
  const { t } = useTranslation();
  return (
    <>
      {snapshot &&
      (!snapshot.enabled ||
        !snapshot.contractReady ||
        snapshot.pendingCount > 0) ? (
        <div className="workspace-notices" role="status">
          {!snapshot.enabled ? <span>{t("memoryPage.disabled")}</span> : null}
          {!snapshot.contractReady ? (
            <span>{t("memoryPage.contractPending")}</span>
          ) : null}
          {snapshot.pendingCount > 0 ? (
            <span>
              {t("memoryPage.pending", { count: snapshot.pendingCount })}
            </span>
          ) : null}
        </div>
      ) : null}
    </>
  );
}
