export const appRoutes = [
  "conversation",
  "memory",
  "work",
  "records",
  "audit",
  "diagnosis",
  "unitTest",
  "settings",
] as const;

export type AppRoute = (typeof appRoutes)[number];
