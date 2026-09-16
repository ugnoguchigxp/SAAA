import { useTranslation } from "react-i18next";
import { AppIcon } from "./AppIcon";

export function ConversationSidebarFooter({
  active,
  settingsActive,
  onOpenSettings,
}: {
  active: boolean;
  settingsActive: boolean;
  onOpenSettings: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="sidebar-footer">
      <div role="status" aria-live="polite" className="conversation-status">
        {t(active ? "app.conversationActive" : "app.conversationIdle")}
      </div>
      <button
        className={settingsActive ? "sidebar-settings active" : "sidebar-settings"}
        onClick={onOpenSettings}
      >
        <AppIcon name="settings" />
        {t("app.settings")}
      </button>
    </div>
  );
}
