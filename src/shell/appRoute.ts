export const appRoutes = [
  "conversation",
  "memory",
  "work",
  "records",
  "audit",
  "settings",
] as const;

export type AppRoute = (typeof appRoutes)[number];
