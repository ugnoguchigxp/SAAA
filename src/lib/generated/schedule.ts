// Generated from src-tauri/src/schedule/contracts.rs. Do not edit.
export type ScheduleEntryView = { id: string, kind: string, subjectRef: string, scopeRef: string, dueAt: number, status: string, origin: string, revision: number, fireResult: string | null, payloadId: string | null, };

export type ScheduleStatus = { enabled: boolean, calendarEnabled: boolean, calendarId: string | null, calendarConnected: boolean, lastError: string | null, platformSupported: boolean, };

export type ScheduleAddInput = { kind: string, subjectRef: string, scopeRef: string, dueAt: number, windowEndAt: number | null, delegationRef: string | null, payload: string | null, };
