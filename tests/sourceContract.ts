import { readFileSync } from "node:fs";
import { join } from "node:path";
/** Structural source guards ignore layout; behavioral tests cover runtime semantics. */
export function containsSource(source: string, fragment: string) {
  return normalize(source).includes(normalize(fragment));
}

function normalize(value: string) {
  return value.replace(/\s+/g, "").replace(/,(?=[})\]])/g, "");
}

export function readProjectSource(path: string) {
  return readFileSync(join(import.meta.dir, "..", path), "utf8");
}
