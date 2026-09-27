import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useState, type RefObject } from "react";
import {
  attachPreviewWebview,
  closePreviewWebview,
  setActiveSourceWebview,
  type PreviewWebviewHandle,
} from "./artifactWebviewHost";
import { logicalRectFromDom } from "./webviewGeometry";

export type SourceStatus = "connecting" | "loading" | "preparing" | "ready" | "error" | "timeout";
export type SourceLoadEvent = {
  navigation: number;
  phase: "loading" | "preparing" | "ready" | "error";
};
export const SOURCE_SLOW_MS = 12_000;
export const SOURCE_TIMEOUT_MS = 45_000;

export function useSourceWebsite(options: {
  conversationId: string;
  url: string;
  hostRef: RefObject<HTMLDivElement | null>;
  retry: number;
  onReady?: (ready: boolean) => void;
}) {
  const { conversationId, url, hostRef, retry, onReady } = options;
  const [status, setStatus] = useState<SourceStatus>("connecting");
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    let closed = false;
    let failed = false;
    let label: string | null = null;
    let webview: PreviewWebviewHandle | null = null;
    let navigation = -1;
    let pageReady = false;
    let frame = 0;
    let positioning = Promise.resolve();
    let slowTimer: ReturnType<typeof setTimeout>;
    let timeoutTimer: ReturnType<typeof setTimeout>;
    const host = hostRef.current;
    const scheduleFrame =
      typeof requestAnimationFrame === "function"
        ? requestAnimationFrame
        : (callback: FrameRequestCallback) => setTimeout(() => callback(0), 0) as unknown as number;
    const cancelFrame =
      typeof cancelAnimationFrame === "function"
        ? cancelAnimationFrame
        : (id: number) => clearTimeout(id);

    function stopTimers() {
      clearTimeout(slowTimer);
      clearTimeout(timeoutTimer);
    }
    async function dispose() {
      if (webview) {
        await webview.hide().catch(() => undefined);
        await webview.close().catch(() => undefined);
      } else if (label) await closePreviewWebview(label).catch(() => undefined);
    }
    function fail(reason: "error" | "timeout") {
      if (closed || failed) return;
      failed = true;
      pageReady = false;
      stopTimers();
      setActiveSourceWebview(null);
      onReady?.(false);
      setStatus(reason);
      void dispose();
    }
    function startTimers() {
      stopTimers();
      setSlow(false);
      slowTimer = setTimeout(() => {
        if (!closed && !failed) setSlow(true);
      }, SOURCE_SLOW_MS);
      timeoutTimer = setTimeout(() => fail("timeout"), SOURCE_TIMEOUT_MS);
    }
    function rect() {
      const bounds = host?.getBoundingClientRect();
      return (
        bounds &&
        logicalRectFromDom(bounds, {
          viewportWidth: window.innerWidth,
          viewportHeight: window.innerHeight,
          hidden: document.hidden,
          clip: host?.closest(".artifact-panel-body")?.getBoundingClientRect(),
        })
      );
    }
    function position() {
      // Serialize native operations so an old resize cannot show a closed/loading tab.
      positioning = positioning
        .then(async () => {
          if (!webview || closed || failed) return;
          const bounds = rect();
          if (!bounds?.visible) {
            await webview.hide();
            return;
          }
          await webview.setPosition(bounds.x, bounds.y);
          await webview.setSize(bounds.width, bounds.height);
          if (closed || failed) return;
          if (!pageReady) {
            await webview.hide();
            return;
          }
          const shownNavigation = navigation;
          await webview.show();
          if (closed || failed || !pageReady || navigation !== shownNavigation) {
            await webview.hide();
            return;
          }
          stopTimers();
          setSlow(false);
          setActiveSourceWebview(webview);
          onReady?.(true);
          setStatus("ready");
        })
        .catch(() => fail("error"));
      return positioning;
    }
    function schedulePosition() {
      cancelFrame(frame);
      frame = scheduleFrame(() => {
        void position();
      });
    }
    const channel = new Channel<SourceLoadEvent>();
    channel.onmessage = (event) => {
      if (closed || failed || event.navigation < navigation) return;
      navigation = event.navigation;
      if (event.phase === "error") {
        fail("error");
        return;
      }
      pageReady = event.phase === "ready";
      if (pageReady) {
        stopTimers();
        setSlow(false);
        setStatus("preparing");
        void position();
      } else {
        setActiveSourceWebview(null);
        onReady?.(false);
        setStatus(event.phase);
        if (event.phase === "loading") startTimers();
        void position();
      }
    };
    async function mount() {
      setStatus("connecting");
      onReady?.(false);
      startTimers();
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
          onLoad: channel,
        });
        if (closed || failed) {
          await dispose();
          return;
        }
        webview = await attachPreviewWebview(label);
        if (closed || failed) {
          await dispose();
          return;
        }
        await position();
      } catch {
        fail("error");
      }
    }
    void mount();
    window.addEventListener("resize", schedulePosition);
    document.addEventListener("visibilitychange", schedulePosition);
    const observer =
      host && typeof ResizeObserver !== "undefined" ? new ResizeObserver(schedulePosition) : null;
    if (host && observer) observer.observe(host);
    return () => {
      closed = true;
      stopTimers();
      cancelFrame(frame);
      window.removeEventListener("resize", schedulePosition);
      document.removeEventListener("visibilitychange", schedulePosition);
      observer?.disconnect();
      setActiveSourceWebview(null);
      onReady?.(false);
      void positioning.finally(dispose);
    };
  }, [conversationId, url, hostRef, retry, onReady]);
  return { status, slow };
}
