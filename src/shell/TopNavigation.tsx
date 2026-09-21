import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { appRoutes, type AppRoute } from "./appRoute";

export function TopNavigation({
  active,
  onChange,
}: {
  active: AppRoute;
  onChange: (route: AppRoute) => void;
}) {
  const { t } = useTranslation();
  const refs = useRef<Array<HTMLButtonElement | null>>([]);

  return (
    <nav className="top-navigation" aria-label={t("navigation.label")}>
      {appRoutes.map((route, index) => (
        <button
          key={route}
          ref={(node) => {
            refs.current[index] = node;
          }}
          type="button"
          className={route === active ? "top-navigation-item active" : "top-navigation-item"}
          aria-current={route === active ? "page" : undefined}
          onClick={() => onChange(route)}
          onKeyDown={(event) => {
            let next = index;
            if (event.key === "ArrowLeft") next = (index - 1 + appRoutes.length) % appRoutes.length;
            else if (event.key === "ArrowRight") next = (index + 1) % appRoutes.length;
            else if (event.key === "Home") next = 0;
            else if (event.key === "End") next = appRoutes.length - 1;
            else return;
            event.preventDefault();
            refs.current[next]?.focus();
          }}
        >
          {t(`navigation.${route}`)}
        </button>
      ))}
    </nav>
  );
}
