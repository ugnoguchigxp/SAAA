import type {
  DiagnosisCapabilityReport,
  DiagnosisReport,
  DiagnosisState,
} from "../../lib/generated/diagnosis";

/** A report older than this is refreshed with a quick scan when the page opens. */
export const STALE_REPORT_MS = 2 * 60 * 1000;

export type AgeUnit = "never" | "now" | "minutes" | "hours" | "days";

export function age(now: number, at: number | null): { unit: AgeUnit; count: number } {
  if (at == null || at <= 0) return { unit: "never", count: 0 };
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  if (seconds < 45) return { unit: "now", count: 0 };
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return { unit: "minutes", count: Math.max(1, minutes) };
  const hours = Math.round(minutes / 60);
  if (hours < 24) return { unit: "hours", count: hours };
  return { unit: "days", count: Math.round(hours / 24) };
}

export type Summary = {
  /** Capabilities the verdict talks about; optional and disabled ones are left out. */
  counted: number;
  ready: number;
  degraded: number;
  unavailable: number;
  unverified: number;
};

export function summarize(report: DiagnosisReport): Summary {
  const summary: Summary = { counted: 0, ready: 0, degraded: 0, unavailable: 0, unverified: 0 };
  for (const item of report.capabilities) {
    if (item.state === "disabled" || (item.optional && item.state === "unverified")) continue;
    summary.counted += 1;
    summary[item.state] += 1;
  }
  return summary;
}

const STATE_ORDER: Record<DiagnosisState, number> = {
  unavailable: 0,
  degraded: 1,
  unverified: 2,
  ready: 3,
  disabled: 4,
};

/** Problems first, then unproven, then healthy. Stable within a rank. */
export function sortCapabilities(
  items: readonly DiagnosisCapabilityReport[],
): DiagnosisCapabilityReport[] {
  return items
    .map((item, index) => ({ item, index }))
    .sort(
      (a, b) =>
        Number(a.item.optional) - Number(b.item.optional) ||
        STATE_ORDER[a.item.state] - STATE_ORDER[b.item.state] ||
        a.index - b.index,
    )
    .map(({ item }) => item);
}

/** i18next treats "." as a separator, so a source id becomes a flat key. */
export function sourceKey(source: string): string {
  return source.replace(/[^A-Za-z0-9]+/g, "_");
}

export function isStale(report: DiagnosisReport | null, now: number): boolean {
  if (!report || report.revision === 0) return true;
  return report.finishedAt == null || now - report.finishedAt > STALE_REPORT_MS;
}
