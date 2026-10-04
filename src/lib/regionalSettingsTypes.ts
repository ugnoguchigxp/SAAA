export type DisplayLanguagePreference = "system" | "en" | "ja";
export type LengthUnitSystem = "metric" | "imperial";
export type WeightUnit = "kilogram" | "pound";
export type CurrencyCode =
  | "JPY"
  | "USD"
  | "EUR"
  | "GBP"
  | "CNY"
  | "KRW"
  | "AUD"
  | "CAD"
  | "CHF"
  | "SGD";
export type RegionalPreferencesSettings = {
  language: DisplayLanguagePreference;
  timeZone: string;
  lengthUnit: LengthUnitSystem;
  weightUnit: WeightUnit;
  currency: CurrencyCode;
};
