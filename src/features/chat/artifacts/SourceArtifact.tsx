import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

type SourceSnapshot = {
  url: string;
  text: string;
  observedAt: number;
  truncated: boolean;
};

export default function SourceArtifact({
  conversationId,
  url,
}: {
  conversationId: string;
  url: string;
}) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<SourceSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setFailed(false);
    setSnapshot(null);
    void invoke<SourceSnapshot | null>("read_source_artifact", { conversationId, url })
      .then((value) => {
        if (active) setSnapshot(value);
      })
      .catch(() => {
        if (active) setFailed(true);
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [conversationId, url]);
  return (
    <section className="artifact-source">
      <p className="artifact-source-url">
        <a href={url} target="_blank" rel="noopener noreferrer">
          {url}
        </a>
      </p>
      {loading ? <p role="status">{t("genui.sourceLoading")}</p> : null}
      {!loading && failed ? <p role="alert">{t("genui.sourceFailed")}</p> : null}
      {!loading && !failed && !snapshot ? <p>{t("genui.sourceUnavailable")}</p> : null}
      {snapshot ? (
        <>
          <p>{t("genui.sourceCapturedAt", { time: new Date(snapshot.observedAt).toLocaleString() })}</p>
          {snapshot.truncated ? <p>{t("genui.sourceTruncated")}</p> : null}
          <pre className="artifact-source-text">{snapshot.text}</pre>
        </>
      ) : null}
    </section>
  );
}
