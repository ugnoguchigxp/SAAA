import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

const statusSchema = z
  .object({
    enabled: z.boolean(),
    calendarEnabled: z.boolean(),
    calendarId: z.string().nullable(),
    calendarConnected: z.boolean(),
    lastError: z.string().nullable(),
    platformSupported: z.boolean(),
  })
  .strict();

export type ScheduleStatus = z.infer<typeof statusSchema>;

export async function loadScheduleStatus(): Promise<ScheduleStatus> {
  return statusSchema.parse(await invoke("schedule_status"));
}

export async function setScheduleEnabled(enabled: boolean): Promise<ScheduleStatus> {
  return statusSchema.parse(await invoke("schedule_set_enabled", { enabled }));
}

export async function setScheduleCalendar(
  enabled: boolean,
  calendarId: string | null,
): Promise<ScheduleStatus> {
  return statusSchema.parse(await invoke("schedule_set_calendar", { enabled, calendarId }));
}
