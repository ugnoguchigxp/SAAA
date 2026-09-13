import { useEffect, useRef, useState } from "react";
export function useUiVisibility() {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [foreground, setForeground] = useState(document.visibilityState !== "hidden");
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new IntersectionObserver(
      (entries) => setVisible(entries.some((e) => e.isIntersecting)),
      { threshold: 0 },
    );
    observer.observe(element);
    const changed = () => setForeground(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", changed);
    return () => {
      observer.disconnect();
      document.removeEventListener("visibilitychange", changed);
    };
  }, []);
  return { ref, active: visible && foreground };
}
