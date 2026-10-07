import { watch, type FSWatcher } from "node:fs";
import { eventPath, isScanExcluded, SCAN_EXCLUDED, type InputScan } from "./verify-input-scan";
import { scanVerificationInputs, trackedInputFiles } from "./verify-fingerprint";

export type WatchDetail = {
  available: boolean;
  generation: number;
  generationStart: number;
  generationEnd: number;
  overflow: boolean;
  failed: boolean;
  scanned: boolean;
  scanMatched: boolean;
  excluded: string[];
  limitation: string;
};

export const WATCH_LIMITATION =
  "開始・終了scanと観測した変更イベントを比較した。OSが通知しなかった一時変更が終了前に完全復元された場合や、観測区間外の変更まで検出する保証はない。";

export function startInputWatch(
  root: string,
  options: { maxEvents?: number; exclude?: (relativePath: string) => boolean } = {},
) {
  const maxEvents = options.maxEvents ?? 10_000;
  const exclude = options.exclude ?? defaultExclude;
  const detail: WatchDetail = {
    available: false,
    generation: 0,
    generationStart: 0,
    generationEnd: 0,
    overflow: false,
    failed: false,
    scanned: false,
    scanMatched: false,
    excluded: [...SCAN_EXCLUDED],
    limitation: WATCH_LIMITATION,
  };
  let baseline: InputScan | undefined;
  let tracked = new Set<string>();
  let closed = false;
  let watcher: FSWatcher | undefined;
  try {
    watcher = watch(root, { recursive: true }, (_event, name) => {
      const relativePath = eventPath(root, name);
      if (!relativePath) {
        detail.failed = true;
        return;
      }
      if (relativePath === ".git" || relativePath.startsWith(".git/")) return;
      const trackedEvent = [...tracked].some(
        (path) => path === relativePath || path.startsWith(`${relativePath}/`),
      );
      if (exclude(relativePath) && !trackedEvent) return;
      detail.generation += 1;
      if (detail.generation >= maxEvents) detail.overflow = true;
    });
    watcher.on("error", () => {
      detail.failed = true;
    });
    detail.available = true;
  } catch {
    detail.available = false;
    detail.failed = true;
  }
  const copy = () => ({ ...detail, excluded: [...detail.excluded] });
  return {
    available: detail.available,
    generation: () => detail.generation,
    overflow: () => detail.overflow,
    failed: () => detail.failed,
    detail: () => ({
      ...copy(),
      generationEnd: detail.scanned ? detail.generation : detail.generationEnd,
    }),
    begin: () => {
      detail.scanned = false;
      detail.scanMatched = false;
      if (closed) detail.failed = true;
      detail.generationStart = detail.generation;
      tracked = trackedInputFiles(root) ?? new Set();
      baseline = scanVerificationInputs(root);
      if (!baseline.ok) detail.failed = true;
      return baseline;
    },
    finalScan: () => {
      if (closed) detail.failed = true;
      const end = scanVerificationInputs(root);
      detail.generationEnd = detail.generation;
      const matched = baseline?.ok === true && end.ok && baseline.digest === end.digest;
      detail.scanned = baseline?.ok === true && end.ok;
      detail.scanMatched = matched;
      if (!end.ok) detail.failed = true;
      return copy();
    },
    close: () => {
      closed = true;
      watcher?.close();
    },
  };
}

export type InputWatch = ReturnType<typeof startInputWatch>;

export function defaultExclude(relativePath: string): boolean {
  return isScanExcluded(relativePath.split("\\").join("/"));
}
