import { lazy } from "react";

export const SettingsPage = lazy(() =>
  import("./features/settings/SettingsPage").then(({ SettingsPage }) => ({
    default: SettingsPage,
  })),
);
