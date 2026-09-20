import { useTranslation } from "react-i18next";
import { useDialogFocus } from "../../components/useDialogFocus";
import { AuditLogPage } from "./AuditLogPage";

export function AuditLogModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { dialogRef, fallbackRef } = useDialogFocus(true, onClose);

  return (
    <div
      ref={fallbackRef}
      className="workspace-modal-backdrop"
      tabIndex={-1}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        ref={dialogRef}
        className="workspace-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-modal-title"
        tabIndex={-1}
      >
        <header className="workspace-modal-header">
          <h2 id="workspace-modal-title">{t("app.audit")}</h2>
          <button type="button" className="text-button" onClick={onClose}>
            {t("common.back")}
          </button>
        </header>
        <div className="workspace-modal-body">
          <AuditLogPage />
        </div>
      </section>
    </div>
  );
}
