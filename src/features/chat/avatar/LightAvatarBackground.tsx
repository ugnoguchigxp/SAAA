import { useEffect, useRef, useState } from "react";
import type { AvatarMotion, LightAvatar } from "./model.js";
import "./lightAvatarBackground.css";

export type AvatarCue = { id: string; motion: AvatarMotion };

export function useAvatarVisibility(active: boolean) {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");
  const [reduced, setReduced] = useState(
    () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false,
  );
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== "hidden");
    const media = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    const reducedChanged = () => setReduced(media?.matches ?? false);
    document.addEventListener("visibilitychange", changed);
    media?.addEventListener("change", reducedChanged);
    return () => {
      document.removeEventListener("visibilitychange", changed);
      media?.removeEventListener("change", reducedChanged);
    };
  }, []);
  return { visible: active && visible, reduced };
}

export function LightAvatarBackground({
  active,
  reduced,
  cue = null,
}: {
  active: boolean;
  reduced: boolean;
  cue?: AvatarCue | null;
}) {
  const host = useRef<HTMLDivElement>(null);
  const play = useRef<((cue: AvatarCue | null) => void) | null>(null);
  const latest = useRef(cue);
  latest.current = cue;
  useEffect(() => {
    const element = host.current;
    if (!active || !element || !window.WebGL2RenderingContext) return;
    let cancelled = false;
    let model: LightAvatar | undefined;
    let observer: ResizeObserver | undefined;
    let frame = 0;
    let motion: AvatarMotion = "neutral";
    let start = 0;
    let lastFrame = 0;
    let elapsed = 0;
    let duration = 8.7;
    let lastCue: string | undefined;
    const stop = () => {
      cancelAnimationFrame(frame);
      frame = 0;
    };
    const failed = () => {
      if (cancelled) return;
      stop();
      observer?.disconnect();
      play.current = null;
      model?.canvas.removeEventListener("webglcontextlost", contextLost);
      model?.dispose();
      model = undefined;
    };
    const draw = () => {
      try {
        model?.render(Math.min(elapsed, 8), motion, elapsed >= 8 ? elapsed - 8 : elapsed);
      } catch {
        failed();
      }
    };
    const tick = (now: number) => {
      if (cancelled || !model) return;
      if (now - lastFrame >= 1000 / 24) {
        lastFrame = now;
        elapsed = Math.min((now - start) / 1000, duration);
        if (elapsed >= 8) {
          // All expressions return to rest with the same 0.7 second blend.
          motion = "neutral";
          try {
            model.render(8, motion, elapsed - 8);
          } catch {
            failed();
          }
        } else draw();
      }
      if (model && elapsed < duration) frame = requestAnimationFrame(tick);
      else {
        elapsed = 0;
        motion = "neutral";
        stop();
      }
    };
    const contextLost = (event: Event) => {
      event.preventDefault();
      failed();
    };
    void import("./model.js")
      .then(({ createLightAvatar }) => {
        if (cancelled) return;
        try {
          model = createLightAvatar(element);
          model.canvas.addEventListener("webglcontextlost", contextLost);
          draw();
          observer = new ResizeObserver(() => {
            if (!model || element.clientWidth === 0 || element.clientHeight === 0) return;
            try {
              model.resize();
              draw();
            } catch {
              failed();
            }
          });
          observer.observe(element);
          play.current = (next) => {
            if (reduced || !model || (next ? next.id === lastCue : lastCue === undefined)) return;
            lastCue = next?.id;
            stop();
            motion = next?.motion ?? "neutral";
            duration = motion === "neutral" ? 0.7 : 8.7;
            model.beginMotion();
            start = performance.now();
            lastFrame = 0;
            elapsed = 0;
            frame = requestAnimationFrame(tick);
          };
          play.current(latest.current);
        } catch {
          failed();
        }
      })
      .catch(failed);
    return () => {
      cancelled = true;
      stop();
      observer?.disconnect();
      play.current = null;
      model?.canvas.removeEventListener("webglcontextlost", contextLost);
      model?.dispose();
    };
  }, [active, reduced]);
  useEffect(() => {
    play.current?.(cue);
  }, [cue]);
  return <div ref={host} className="light-avatar-background" aria-hidden="true" />;
}
