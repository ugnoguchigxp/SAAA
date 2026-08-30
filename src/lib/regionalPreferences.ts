import type {
  CurrencyCode,
  DisplayLanguagePreference,
  LengthUnitSystem,
  WeightUnit,
} from "./settingsTypes";

export const DISPLAY_LANGUAGE_PREFERENCES = ["system", "en", "ja"] as const satisfies readonly DisplayLanguagePreference[];
export const LENGTH_UNIT_SYSTEMS = ["metric", "imperial"] as const satisfies readonly LengthUnitSystem[];
export const WEIGHT_UNITS = ["kilogram", "pound"] as const satisfies readonly WeightUnit[];
export const CURRENCY_CODES = ["JPY", "USD", "EUR", "GBP", "CNY", "KRW", "AUD", "CAD", "CHF", "SGD"] as const satisfies readonly CurrencyCode[];

export function isSupportedTimeZone(value: string): boolean {
  if (value === "system") return true;
  if (value.length > 100 || !/^[A-Za-z0-9._+-]+(?:\/[A-Za-z0-9._+-]+)*$/.test(value)) return false;
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

export function systemTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

export function availableTimeZones(): string[] {
  const supportedValuesOf = (Intl as typeof Intl & {
    supportedValuesOf?: (key: "timeZone") => string[];
  }).supportedValuesOf;
  const supported = supportedValuesOf?.("timeZone") ?? [];
  return [...new Set([systemTimeZone(), "UTC", ...supported])].sort();
}
