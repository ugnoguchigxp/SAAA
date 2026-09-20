import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  loadScheduleStatus,
  setScheduleCalendar,
  setScheduleEnabled,
  type ScheduleStatus,
} from "../../lib/scheduleApi";

const defaultStatus: ScheduleStatus = {
  enabled: false,
  calendarEnabled: false,
  calendarId: null,
  calendarConnected: false,
  lastError: null,
  platformSupported: true,
};

export function ScheduleSection() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<ScheduleStatus>(defaultStatus);
  const [calendarId, setCalendarId] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void loadScheduleStatus()
      .then((next) => {
        setStatus(next);
        setCalendarId(next.calendarId ?? "");
      })
      .catch((cause) => setError(String(cause)));
  }, []);

  async function toggleEnabled(enabled: boolean) {
    setError(null);
    try {
      setStatus(await setScheduleEnabled(enabled));
    } catch (cause) {
      setError(String(cause));
    }
  }

  async function saveCalendar(enabled: boolean) {
    setError(null);
    try {
      setStatus(await setScheduleCalendar(enabled, calendarId.trim() || null));
    } catch (cause) {
      setError(String(cause));
    }
  }

  return (
    <section className="settings-section" aria-label={t("settings.schedule.title")}>
      <h3>{t("settings.schedule.title")}</h3>
      <p className="settings-help">{t("settings.schedule.help")}</p>
      <label className="settings-switch">
        <input
          type="checkbox"
          checked={status.enabled}
          onChange={(event) => void toggleEnabled(event.target.checked)}
        />
        <span>{t("settings.schedule.enable")}</span>
      </label>
      <label className="settings-switch">
        <input
          type="checkbox"
          checked={status.calendarEnabled}
          disabled={!status.platformSupported}
          onChange={(event) => void saveCalendar(event.target.checked)}
        />
        <span>{t("settings.schedule.calendar")}</span>
      </label>
      <label className="settings-field">
        <span>{t("settings.schedule.calendarId")}</span>
        <input
          value={calendarId}
          onChange={(event) => setCalendarId(event.target.value)}
          onBlur={() => {
            if (status.calendarEnabled) void saveCalendar(true);
          }}
        />
      </label>
      <p className="settings-help">
        {status.calendarConnected
          ? t("settings.schedule.connected")
          : t("settings.schedule.disconnected")}
      </p>
      {status.lastError ? (
        <p className="save-error" role="status">
          {t("settings.schedule.lastError", { code: status.lastError })}
        </p>
      ) : null}
      {error ? <p className="save-error">{error}</p> : null}
    </section>
  );
}
