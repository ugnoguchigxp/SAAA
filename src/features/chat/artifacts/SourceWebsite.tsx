import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { attachPreviewWebview, closePreviewWebview, setActiveSourceWebview, type PreviewWebviewHandle } from "./artifactWebviewHost";
import { isStaleGeometry, logicalRectFromDom, nextGeometryGeneration } from "./webviewGeometry";

export default function SourceWebsite({ conversationId, url, title, onReady }: {
  conversationId: string;
  url: string;
  title: string;
  onReady?: (ready: boolean) => void;
}) {
  const { t } = useTranslation();
  const hostRef = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");

  useEffect(() => {
    let closed = false;
    let label: string | null = null;
    let webview: PreviewWebviewHandle | null = null;
    let frame = 0;
    let generation = 0;
    const host = hostRef.current;
    const scheduleFrame = typeof requestAnimationFrame === "function"
      ? requestAnimationFrame
      : (callback: FrameRequestCallback) => setTimeout(() => callback(0), 0) as unknown as number;
    const cancelFrame = typeof cancelAnimationFrame === "function"
      ? cancelAnimationFrame
      : (id: number) => clearTimeout(id);

    function rect() {
      const bounds = host?.getBoundingClientRect();
      return bounds && logicalRectFromDom(bounds, {
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        hidden: document.hidden,
        clip: host?.closest(".artifact-panel-body")?.getBoundingClientRect(),
      });
    }

    async function position() {
      const applied = generation;
      const bounds = rect();
      if (!webview || !bounds || closed || isStaleGeometry(applied, generation)) return;
      if (!bounds.visible) {
        await webview.hide();
        return;
      }
      await webview.setPosition(bounds.x, bounds.y);
      await webview.setSize(bounds.width, bounds.height);
      if (closed || isStaleGeometry(applied, generation)) return;
      await webview.show();
    }

    function schedulePosition() {
      generation = nextGeometryGeneration(generation);
      cancelFrame(frame);
      frame = scheduleFrame(() => void position().catch(() => setStatus("error")));
    }

    async function mount() {
      setStatus("loading");
      try {
        const bounds = rect();
        if (!bounds?.visible) throw new Error("source-geometry");
        label = await invoke<string>("mount_source_website", {
          conversationId,
          url,
          x: bounds.x,
          y: bounds.y,
          width: bounds.width,
          height: bounds.height,
        });
        if (closed) {
          await closePreviewWebview(label);
          return;
        }
        webview = await attachPreviewWebview(label);
        if (closed) {
          await webview.close();
          return;
        }
        await position();
        setActiveSourceWebview(webview);
        onReady?.(true);
        setStatus("ready");
      } catch {
        if (label) await closePreviewWebview(label).catch(() => undefined);
        if (!closed) {
          onReady?.(false);
          setStatus("error");
        }
      }
    }

    void mount();
    window.addEventListener("resize", schedulePosition);
    document.addEventListener("visibilitychange", schedulePosition);
    const observer = host && typeof ResizeObserver !== "undefined"
      ? new ResizeObserver(schedulePosition) : null;
    if (host && observer) observer.observe(host);
    return () => {
      closed = true;
      cancelFrame(frame);
      window.removeEventListener("resize", schedulePosition);
      document.removeEventListener("visibilitychange", schedulePosition);
      observer?.disconnect();
      setActiveSourceWebview(null);
      onReady?.(false);
      if (webview) void webview.close().catch(() => undefined);
      else if (label) void closePreviewWebview(label).catch(() => undefined);
    };
  }, [conversationId, url, onReady]);

  return (
    <div ref={hostRef} className="artifact-webview-host" aria-label={title}>
      {status === "loading" && <p role="status">{t("genui.sourceWebsiteLoading")}</p>}
      {status === "error" && <p role="alert">{t("genui.sourceWebsiteFailed")}</p>}
      {status === "ready" && <p className="visually-hidden">{t("genui.sourceTabReload")}</p>}
    </div>
  );
}
