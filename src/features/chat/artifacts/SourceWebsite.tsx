import { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSourceWebsite } from "./useSourceWebsite";

export default function SourceWebsite({
  conversationId,
  url,
  title,
  onReady,
}: {
  conversationId: string;
  url: string;
  title: string;
  onReady?: (ready: boolean) => void;
}) {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const headingId = useId();
  const [retry, setRetry] = useState(0);
  const { status, slow } = useSourceWebsite({ conversationId, url, hostRef, retry, onReady });
  const failed = status === "error" || status === "timeout";
  const message =
    status === "timeout"
      ? "genui.sourceWebsiteTimeout"
      : status === "error"
        ? "genui.sourceWebsiteFailed"
        : status === "loading"
          ? "genui.sourceWebsiteReceiving"
          : status === "preparing"
            ? "genui.sourceWebsitePreparing"
            : "genui.sourceWebsiteConnecting";
  return (
    <div
      ref={hostRef}
      className="artifact-webview-host"
      aria-label={title}
      aria-busy={!failed && status !== "ready"}
    >
      {status !== "ready" && (
        <div className="artifact-source-loading">
          <section
            role="dialog"
            aria-labelledby={headingId}
            className="artifact-source-loading-card"
          >
            <h3 id={headingId}>{failed ? title : t("genui.sourceWebsiteLoading")}</h3>
            {!failed && <progress aria-label={t(message)} />}
            <p role={failed ? "alert" : "status"}>{t(message)}</p>
            {slow && !failed && <p>{t("genui.sourceWebsiteSlow")}</p>}
            {(failed || slow) && (
              <button type="button" onClick={() => setRetry((value) => value + 1)}>
                {t("genui.sourceWebsiteRetry")}
              </button>
            )}
          </section>
        </div>
      )}
      {status === "ready" && <p className="visually-hidden">{t("genui.sourceTabReload")}</p>}
    </div>
  );
}
