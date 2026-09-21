import type { ReactNode } from "react";
import type { AppRoute } from "./appRoute";
import { TopNavigation } from "./TopNavigation";
import "./appShell.css";

export function AppShell({
  route,
  onRouteChange,
  children,
}: {
  route: AppRoute;
  onRouteChange: (route: AppRoute) => void;
  children: ReactNode;
}) {
  return (
    <section className="app-primary-region">
      <TopNavigation active={route} onChange={onRouteChange} />
      <div className="app-page-region">{children}</div>
    </section>
  );
}
