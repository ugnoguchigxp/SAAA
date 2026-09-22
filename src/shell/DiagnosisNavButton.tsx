import { useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { DiagnosisModal } from "../features/diagnosis/DiagnosisModal";

export function DiagnosisNavButton() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="top-navigation-item" onClick={() => setOpen(true)}>
        {t("chat.diagnosis.open")}
      </button>
      {open && createPortal(<DiagnosisModal onClose={() => setOpen(false)} />, document.body)}
    </>
  );
}
