export const appRoutes = [
  "conversation",
  "memory",
  "work",
  "records",
  "audit",
  "diagnosis",
  "unitTest",
  "ttsDictionary",
  "settings",
] as const;

export type AppRoute = (typeof appRoutes)[number];
