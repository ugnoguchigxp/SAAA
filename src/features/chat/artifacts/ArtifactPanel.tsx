import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { UiInstance, UiViewRevision } from "../../../lib/generated/generativeUi";
import { uiApi } from "../ui/api";
import { UiContext } from "../ui/context";
import { uiStates } from "../ui/instanceState";
import { listUiViewRevisions } from "../ui/revisionsApi";
import { useGenUiEnabled } from "../ui/settings";
import { SemanticRenderer } from "../ui/SemanticRenderer";
import { UiBoundary } from "../ui/UiBoundary";
import { lineDiff } from "./lineDiff";

function revisionText(instance: UiInstance): string {
  return instance.node.kind === "Markdown"
    ? (instance.node.args[0] ?? "")
    : JSON.stringify(instance.node, null, 2);
}

export default function ArtifactPanel({
  instance: initialInstance,
  conversationId,
}: {
  instance: UiInstance;
  conversationId: string;
}) {
  const { t } = useTranslation();
  const enabled = useGenUiEnabled();
  const currentRevision = useMemo<UiViewRevision>(
    () => ({
      revision: initialInstance.revision,
      summary: initialInstance.summary,
      createdAt: "",
    }),
    [initialInstance.revision, initialInstance.summary],
  );
  const [revisions, setRevisions] = useState<UiViewRevision[]>([currentRevision]);
  const [revision, setRevision] = useState(initialInstance.revision);
  const [instance, setInstance] = useState(initialInstance);
  const loadedRevisionRef = useRef(initialInstance.revision);
  const [previous, setPrevious] = useState<UiInstance | null>(null);
  const [showDiff, setShowDiff] = useState(false);
  const [revisionLoadFailed, setRevisionLoadFailed] = useState(false);
  const [instanceLoadFailed, setInstanceLoadFailed] = useState(false);
  const [loadingRevision, setLoadingRevision] = useState(false);

  useEffect(() => {
    let active = true;
    setRevision(initialInstance.revision);
    setInstance(initialInstance);
    loadedRevisionRef.current = initialInstance.revision;
    setRevisions([currentRevision]);
    setPrevious(null);
    setShowDiff(false);
    setRevisionLoadFailed(false);
    setInstanceLoadFailed(false);
    void listUiViewRevisions(initialInstance.viewId)
      .then((value) => active && setRevisions(value.length ? value : [currentRevision]))
      .catch(() => active && setRevisionLoadFailed(true));
    return () => {
      active = false;
    };
  }, [currentRevision, initialInstance]);

  useEffect(() => {
    let active = true;
    let release: (() => void) | undefined;
    if (revision === initialInstance.revision) {
      release = uiStates.retain(initialInstance);
      loadedRevisionRef.current = initialInstance.revision;
      setInstance(initialInstance);
      setLoadingRevision(false);
      return () => {
        active = false;
        release?.();
      };
    }
    setLoadingRevision(true);
    void uiApi
      .load(initialInstance.id, revision)
      .then((value) => {
        if (!active) return;
        release = uiStates.retain(value);
        loadedRevisionRef.current = value.revision;
        setInstance(value);
      })
      .catch(() => {
        if (!active) return;
        setInstanceLoadFailed(true);
        setRevision(loadedRevisionRef.current);
      })
      .finally(() => {
        if (active) setLoadingRevision(false);
      });
    return () => {
      active = false;
      release?.();
    };
  }, [initialInstance, revision]);

  useEffect(() => {
    if (!showDiff) {
      setPrevious(null);
      return;
    }
    const index = revisions.findIndex((item) => item.revision === revision);
    const previousRevision = revisions[index + 1]?.revision;
    if (!previousRevision) {
      setPrevious(null);
      return;
    }
    let active = true;
    setPrevious(null);
    void uiApi
      .load(initialInstance.id, previousRevision)
      .then((value) => active && setPrevious(value))
      .catch(() => active && setInstanceLoadFailed(true));
    return () => {
      active = false;
    };
  }, [initialInstance.id, revision, revisions, showDiff]);

  const diff = useMemo(
    () => (previous ? lineDiff(revisionText(previous), revisionText(instance)) : []),
    [instance, previous],
  );

  return (
    <UiContext.Provider value={{ instance, conversationId, active: true, enabled }}>
      <div className="artifact-toolbar">
        <label>
          {t("genui.revision")}
          <select
            value={revision}
            onChange={(event) => {
              setInstanceLoadFailed(false);
              setRevision(Number(event.target.value));
            }}
          >
            {revisions.map((item) => (
              <option key={item.revision} value={item.revision}>
                v{item.revision} · {item.summary}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className="secondary-button"
          aria-pressed={showDiff}
          disabled={!revisions.some((item) => item.revision < revision)}
          onClick={() => setShowDiff((value) => !value)}
        >
          {t("genui.diff")}
        </button>
      </div>
      {(revisionLoadFailed || instanceLoadFailed) && (
        <p role="alert" className="artifact-load-error">
          {t("genui.operationFailed")}
        </p>
      )}
      {loadingRevision && <p role="status">{t("genui.loading")}</p>}
      {showDiff && previous ? (
        <pre className="artifact-diff" aria-label={t("genui.diff")}>
          {diff.map((line, index) => (
            <span key={`${index}:${line.kind}`} className={`artifact-diff-${line.kind}`}>
              {line.kind === "added" ? "+ " : line.kind === "removed" ? "- " : "  "}
              {line.text || " "}
              {"\n"}
            </span>
          ))}
        </pre>
      ) : (
        <UiBoundary fallback={<p>{t("genui.unavailable")}</p>}>
          <SemanticRenderer node={instance.node} displayMode="artifact" />
        </UiBoundary>
      )}
    </UiContext.Provider>
  );
}
