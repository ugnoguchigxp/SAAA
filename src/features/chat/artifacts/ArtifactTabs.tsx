import { useTranslation } from "react-i18next";
import { AppIcon } from "../../../components/AppIcon";
import { artifactTabId, artifactTabTitle, type ArtifactTab } from "./artifactTab";

export function ArtifactTabs({
  tabs,
  activeTabId,
  onSelect,
  onClose,
}: {
  tabs: ArtifactTab[];
  activeTabId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="artifact-tabs" role="tablist">
      {tabs.map((tab, index) => {
        const title = artifactTabTitle(tab);
        const tabId = artifactTabId(tab);
        const selected = tabId === activeTabId;
        return (
          <div className="artifact-tab" key={tabId}>
            <button
              type="button"
              role="tab"
              id={`artifact-tab-${index}`}
              aria-controls="artifact-panel-content"
              aria-selected={selected}
              tabIndex={selected ? 0 : -1}
              onClick={() => onSelect(tabId)}
              onKeyDown={(event) => {
                let nextIndex: number | null = null;
                if (event.key === "ArrowLeft") nextIndex = (index - 1 + tabs.length) % tabs.length;
                if (event.key === "ArrowRight") nextIndex = (index + 1) % tabs.length;
                if (event.key === "Home") nextIndex = 0;
                if (event.key === "End") nextIndex = tabs.length - 1;
                if (nextIndex === null) return;
                event.preventDefault();
                const next = tabs[nextIndex];
                onSelect(artifactTabId(next));
                document.getElementById(`artifact-tab-${nextIndex}`)?.focus();
              }}
            >
              {title}
            </button>
            <button
              type="button"
              className="artifact-tab-close"
              aria-label={`${t("genui.close")}: ${title}`}
              title={`${t("genui.close")}: ${title}`}
              onClick={() => onClose(tabId)}
            >
              <AppIcon name="close" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
