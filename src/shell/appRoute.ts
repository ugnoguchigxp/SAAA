const workspaceRoutes = ["conversation", "memory", "work", "records"] as const;
const systemRoutes = ["audit", "diagnosis", "unitTest", "ttsDictionary", "settings"] as const;

export const appRoutes = [...workspaceRoutes, ...systemRoutes] as const;
export type AppRoute = (typeof appRoutes)[number];
