import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useArtifactWebview } from "./useArtifactWebview";

function previewReason(message: string): string {
  const match = message.match(/preview-[a-z-]+/);
  return match?.[0] ?? "preview-failed";
}

export default function InteractivePreview({
  artifactId,
  revisionId,
  title,
}: {
  artifactId: string;
  revisionId: string;
  title: string;
}) {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const [retryNonce, setRetryNonce] = useState(0);
  const [retried, setRetried] = useState(false);
  const { status, message } = useArtifactWebview({
    active: true,
    artifactId,
    revisionId,
    hostRef,
    retryNonce,
  });
  const canRetry = status === "error" && !retried;
  return (
    <div
      ref={hostRef}
      className="artifact-webview-host"
      aria-label={t("genui.previewRegion", { title })}
    >
      {status !== "ready" ? (
        <div className="artifact-preview-status">
          {status === "loading" ? <p>{t("genui.previewLoading")}</p> : null}
          {status === "unavailable" ? <p role="alert">{t("genui.previewUnavailable")}</p> : null}
          {status === "error" ? (
            <p role="alert">
              {t("genui.previewFailed")}
              {canRetry ? (
                <button
                  type="button"
                  onClick={() => {
                    setRetried(true);
                    setRetryNonce((value) => value + 1);
                  }}
                >
                  {t("genui.retry")}
                </button>
              ) : null}
            </p>
          ) : null}
          <p className="artifact-preview-focus">{t("genui.previewFocusHint")}</p>
          {message ? <p className="artifact-load-error">{previewReason(message)}</p> : null}
        </div>
      ) : (
        <p className="visually-hidden">{t("genui.previewReady", { title })}</p>
      )}
    </div>
  );
}
