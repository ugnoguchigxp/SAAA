export type AppIconName = "calendar" | "chat" | "mic" | "model" | "send" | "settings" | "situation" | "stop";

export function AppIcon({ name }: { name: AppIconName }) {
  const common = { width: 20, height: 20, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 1.8, strokeLinecap: "round" as const, strokeLinejoin: "round" as const, "aria-hidden": true };
  switch (name) {
    case "calendar":
      return <svg {...common}><path d="M7 3v3M17 3v3M4 9h16" /><rect x="4" y="5" width="16" height="16" rx="3" /></svg>;
    case "chat":
      return <svg {...common}><path d="M20 15a4 4 0 0 1-4 4H9l-5 3 1.5-4.5A8 8 0 1 1 20 15Z" /></svg>;
    case "mic":
      return <svg {...common}><rect x="8" y="3" width="8" height="13" rx="4" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6" /></svg>;
    case "model":
      return <svg {...common}><path d="M8 4a4 4 0 0 0-3 6.7A4.5 4.5 0 0 0 8.5 19H10V5.5A1.5 1.5 0 0 0 8.5 4ZM16 4a4 4 0 0 1 3 6.7A4.5 4.5 0 0 1 15.5 19H14V5.5A1.5 1.5 0 0 1 15.5 4Z" /><path d="M6 12h4M14 12h4" /></svg>;
    case "send":
      return <svg {...common}><path d="M12 20V5M6 11l6-6 6 6" /></svg>;
    case "settings":
      return <svg {...common}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1a1.7 1.7 0 0 0 1.9.3A1.7 1.7 0 0 0 10 3V2.8h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" /></svg>;
    case "situation":
      return <svg {...common}><circle cx="12" cy="12" r="9" /><path d="M12 3v9h9" /></svg>;
    case "stop":
      return <svg {...common}><rect x="7" y="7" width="10" height="10" rx="2" /></svg>;
  }
}
