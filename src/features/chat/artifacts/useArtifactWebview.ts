import { useEffect, useRef, useState, type RefObject } from "react";
import { artifactPreviewApi } from "./artifactPreviewApi";
import { attachPreviewWebview, closePreviewWebview, type PreviewWebviewHandle } from "./artifactWebviewHost";
import {
  isStaleGeometry,
  logicalRectFromDom,
  nextGeometryGeneration,
  type LogicalRect,
} from "./webviewGeometry";

export type PreviewStatus = "loading" | "ready" | "error" | "unavailable";

const CREATE_TIMEOUT_MS = 5_000;

export function useArtifactWebview(options: {
  active: boolean;
  artifactId: string;
  revisionId: string;
  hostRef: RefObject<HTMLElement | null>;
  retryNonce: number;
}) {
  const [status, setStatus] = useState<PreviewStatus>("loading");
  const [message, setMessage] = useState("");
  const webviewRef = useRef<PreviewWebviewHandle | null>(null);
  const tokenRef = useRef<string | null>(null);
  const labelRef = useRef<string | null>(null);

  useEffect(() => {
    if (!options.active) {
      setStatus("loading");
      return;
    }
    let cancelled = false;
    let generation = 0;
    let frame = 0;
    let waitFrame = 0;
    const host = options.hostRef.current;

    async function teardown() {
      const webview = webviewRef.current;
      webviewRef.current = null;
      const token = tokenRef.current;
      tokenRef.current = null;
      const label = labelRef.current;
      labelRef.current = null;
      if (webview) {
        try {
          await webview.hide();
        } catch {
          /* close still runs */
        }
        try {
          await webview.close();
        } catch {
          /* rust expiry recovers orphans */
        }
      } else if (label) {
        try {
          await closePreviewWebview(label);
        } catch {
          /* rust expiry recovers orphans */
        }
      }
      if (token) {
        try {
          await artifactPreviewApi.release({ previewToken: token });
        } catch {
          /* idempotent */
        }
      }
    }

    function measure(): LogicalRect | null {
      const node = options.hostRef.current;
      if (!node) return null;
      const rect = node.getBoundingClientRect();
      return logicalRectFromDom(rect, {
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        hidden: document.hidden,
      });
    }

    async function applyGeometry(rect: LogicalRect, applied: number) {
      const webview = webviewRef.current;
      if (!webview || cancelled || isStaleGeometry(applied, generation)) return;
      try {
        if (!rect.visible) {
          await webview.hide();
          return;
        }
        await webview.setPosition(rect.x, rect.y);
        await webview.setSize(rect.width, rect.height);
        await webview.show();
      } catch {
        try {
          await webview.hide();
        } catch {
          /* keep UI closed rather than drift */
        }
        if (!cancelled) {
          setMessage("preview-geometry");
          setStatus("error");
        }
      }
    }

    const scheduleFrame =
      typeof requestAnimationFrame === "function"
        ? (callback: FrameRequestCallback) => requestAnimationFrame(callback)
        : (callback: FrameRequestCallback) =>
            setTimeout(() => callback(0), 0) as unknown as number;
    const cancelFrame =
      typeof cancelAnimationFrame === "function"
        ? (id: number) => cancelAnimationFrame(id)
        : (id: number) => clearTimeout(id);

    function waitForRect(): Promise<LogicalRect> {
      return new Promise((resolve) => {
        let attempts = 0;
        const tick = () => {
          if (cancelled) {
            resolve({ x: 0, y: 0, width: 1, height: 1, visible: false });
            return;
          }
          const rect = measure();
          if ((rect && rect.visible) || attempts >= 8) {
            resolve(rect ?? { x: 0, y: 0, width: 0, height: 0, visible: false });
            return;
          }
          attempts += 1;
          waitFrame = scheduleFrame(tick);
        };
        waitFrame = scheduleFrame(tick);
      });
    }

    function scheduleGeometry() {
      generation = nextGeometryGeneration(generation);
      const applied = generation;
      cancelFrame(frame);
      frame = scheduleFrame(() => {
        const rect = measure();
        if (rect) void applyGeometry(rect, applied);
      });
    }

    async function start() {
      setStatus("loading");
      setMessage("");
      try {
        const descriptor = await artifactPreviewApi.prepare({
          artifactId: options.artifactId,
          revisionId: options.revisionId,
        });
        if (cancelled) {
          await artifactPreviewApi.release({ previewToken: descriptor.previewToken });
          return;
        }
        tokenRef.current = descriptor.previewToken;
        labelRef.current = descriptor.webviewLabel;
        const rect = await waitForRect();
        if (cancelled) {
          await teardown();
          return;
        }
        if (!rect.visible) throw new Error("preview-geometry");
        await withTimeout(
          artifactPreviewApi.mount({
            previewToken: descriptor.previewToken,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
          }),
          CREATE_TIMEOUT_MS,
        );
        if (cancelled) {
          await teardown();
          return;
        }
        const webview = await withTimeout(
          attachPreviewWebview(descriptor.webviewLabel),
          CREATE_TIMEOUT_MS,
        );
        if (cancelled) {
          await webview.close();
          await teardown();
          return;
        }
        webviewRef.current = webview;
        scheduleGeometry();
        await webview.show();
        setStatus("ready");
      } catch (error) {
        await teardown();
        if (cancelled) return;
        const code = previewCode(error);
        setMessage(code);
        setStatus(code === "preview-unavailable" ? "unavailable" : "error");
      }
    }

    void start();
    const onResize = () => scheduleGeometry();
    const onVisibility = () => scheduleGeometry();
    window.addEventListener("resize", onResize);
    window.addEventListener("scroll", onResize, true);
    document.addEventListener("visibilitychange", onVisibility);
    const observer = host && typeof ResizeObserver !== "undefined" ? new ResizeObserver(onResize) : null;
    if (host && observer) observer.observe(host);

    return () => {
      cancelled = true;
      cancelFrame(waitFrame);
      cancelFrame(frame);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("scroll", onResize, true);
      document.removeEventListener("visibilitychange", onVisibility);
      observer?.disconnect();
      void teardown();
    };
  }, [options.active, options.artifactId, options.revisionId, options.hostRef, options.retryNonce]);

  return { status, message };
}

function previewCode(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  return text.match(/preview-[a-z-]+/)?.[0] ?? "preview-failed";
}

function withTimeout<T>(work: Promise<T>, timeoutMs: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("preview-create-timeout")), timeoutMs);
    work.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}
