import { useId, type ReactNode } from "react";

export function SituationTabs({
  view,
  onChange,
  labels,
  children,
}: {
  view: "overview" | "review";
  onChange: (view: "overview" | "review") => void;
  labels: { overview: string; review: string };
  children: ReactNode;
}) {
  const id = useId();
  const views = ["overview", "review"] as const;
  return (
    <>
      <div
        className="feedback-row"
        role="tablist"
        aria-label={`${labels.overview} / ${labels.review}`}
      >
        {views.map((name, index) => (
          <button
            key={name}
            id={`${id}-${name}`}
            role="tab"
            aria-selected={view === name}
            aria-controls={`${id}-panel`}
            tabIndex={view === name ? 0 : -1}
            className={view === name ? "selected" : ""}
            onClick={() => onChange(name)}
            onKeyDown={(event) => {
              const next =
                event.key === "Home"
                  ? 0
                  : event.key === "End"
                    ? views.length - 1
                    : event.key === "ArrowRight"
                      ? (index + 1) % views.length
                      : event.key === "ArrowLeft"
                        ? (index + views.length - 1) % views.length
                        : null;
              if (next === null) return;
              event.preventDefault();
              onChange(views[next]);
              document.getElementById(`${id}-${views[next]}`)?.focus();
            }}
          >
            {labels[name]}
          </button>
        ))}
      </div>
      <div id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${view}`} tabIndex={0}>
        {children}
      </div>
    </>
  );
}
