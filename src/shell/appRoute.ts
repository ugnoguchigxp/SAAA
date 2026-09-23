export const appRoutes = [
  "conversation",
  "memory",
  "work",
  "records",
  "audit",
  "diagnosis",
  "settings",
] as const;

export type AppRoute = (typeof appRoutes)[number];
