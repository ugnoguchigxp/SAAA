import { useEffect, useId, useRef, useState, type ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { AppIcon } from "../../components/AppIcon";
import "./ChatOverflowMenu.css";

type WorkspaceModal = ComponentType<{ onClose: () => void }>;

export function ChatOverflowMenu() {
  const { t } = useTranslation();
  const menuId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [workspace, setWorkspace] = useState<"audit" | null>(null);
  const [WorkspaceModal, setWorkspaceModal] = useState<WorkspaceModal | null>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const close = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || workspace) return;
      event.preventDefault();
      setMenuOpen(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [menuOpen, workspace]);

  useEffect(() => {
    if (workspace !== "audit") {
      setWorkspaceModal(null);
      return;
    }
    let cancelled = false;
    void import("../audit/AuditLogModal").then((module) => {
      if (cancelled) return;
      setWorkspaceModal(() => module.AuditLogModal);
    });
    return () => {
      cancelled = true;
      setWorkspaceModal(null);
    };
  }, [workspace]);

  return (
    <div ref={rootRef} className="overflow-menu">
      <button
        type="button"
        className="overflow-menu-button"
        aria-label={t("chat.menuLabel")}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        aria-controls={menuId}
        onClick={() => setMenuOpen((open) => !open)}
      >
        <AppIcon name="menu" />
      </button>
      {menuOpen ? (
        <div id={menuId} className="overflow-menu-panel" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setMenuOpen(false);
              setWorkspace("audit");
            }}
          >
            {t("app.audit")}
          </button>
        </div>
      ) : null}
      {workspace === "audit" && WorkspaceModal ? (
        <WorkspaceModal onClose={() => setWorkspace(null)} />
      ) : null}
    </div>
  );
}
