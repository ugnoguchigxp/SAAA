import { useTranslation } from "react-i18next";
import type { WebSocketConnectionState } from "../lib/contracts";
import { AppIcon } from "./AppIcon";
import "./WebSocketConnectionIndicator.css";

const statusTranslationKeys = {
  connected: "app.webSocketConnected",
  connecting: "app.webSocketConnecting",
  disconnected: "app.webSocketDisconnected",
} as const;

export function WebSocketConnectionIndicator({
  state,
  label,
  statusLabel,
}: {
  state: WebSocketConnectionState;
  label: string;
  statusLabel: string;
}) {
  return <div
    className={`websocket-indicator ${state}`}
    role="status"
    aria-live="polite"
    aria-label={`${label}: ${statusLabel}`}
  >
    <span className="websocket-status-dot" aria-hidden="true" />
    <span className="websocket-status-copy">
      <span>{label}</span>
      <strong>{statusLabel}</strong>
    </span>
  </div>;
}

export function WebSocketSidebarFooter({
  state,
  settingsActive,
  onOpenSettings,
}: {
  state: WebSocketConnectionState;
  settingsActive: boolean;
  onOpenSettings: () => void;
}) {
  const { t } = useTranslation();
  return <div className="sidebar-footer">
    <WebSocketConnectionIndicator
      state={state}
      label={t("app.webSocket")}
      statusLabel={t(statusTranslationKeys[state])}
    />
    <button className={settingsActive ? "sidebar-settings active" : "sidebar-settings"} onClick={onOpenSettings}>
      <AppIcon name="settings" />{t("app.settings")}
    </button>
  </div>;
}
