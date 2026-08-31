import { lazy } from "react";

export const SettingsPage = lazy(() => import("./features/settings/SettingsPage").then(({ SettingsPage }) => ({ default: SettingsPage })));
export const SituationPage = lazy(() => import("./features/situation/SituationPage").then(({ SituationPage }) => ({ default: SituationPage })));
export const AuditLogPage = lazy(() => import("./features/audit/AuditLogPage").then(({ AuditLogPage }) => ({ default: AuditLogPage })));
