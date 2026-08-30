use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const LANGUAGES: &[&str] = &["system", "en", "ja"];
const LENGTH_UNITS: &[&str] = &["metric", "imperial"];
const WEIGHT_UNITS: &[&str] = &["kilogram", "pound"];
const CURRENCIES: &[&str] = &[
    "JPY", "USD", "EUR", "GBP", "CNY", "KRW", "AUD", "CAD", "CHF", "SGD",
];

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegionalPreferences {
    pub(crate) language: String,
    pub(crate) time_zone: String,
    pub(crate) length_unit: String,
    pub(crate) weight_unit: String,
    pub(crate) currency: String,
}

impl Default for RegionalPreferences {
    fn default() -> Self {
        Self {
            language: "system".to_string(),
            time_zone: "system".to_string(),
            length_unit: "metric".to_string(),
            weight_unit: "kilogram".to_string(),
            currency: "JPY".to_string(),
        }
    }
}

pub(super) fn default_value() -> Value {
    serde_json::to_value(RegionalPreferences::default())
        .expect("default regional preferences serialize")
}

pub(crate) fn load(connection: &Connection) -> Result<RegionalPreferences, String> {
    let document = super::read_settings_document(connection, "ui.preferences", "default")?;
    decode(document.value_json)
}

pub(super) fn validate(value: Value) -> Result<(), String> {
    decode(value).map(|_| ())
}

fn decode(value: Value) -> Result<RegionalPreferences, String> {
    let preferences = serde_json::from_value::<RegionalPreferences>(value)
        .map_err(|error| format!("Invalid regional preferences: {error}"))?;
    if !LANGUAGES.contains(&preferences.language.as_str())
        || !valid_time_zone(&preferences.time_zone)
        || !LENGTH_UNITS.contains(&preferences.length_unit.as_str())
        || !WEIGHT_UNITS.contains(&preferences.weight_unit.as_str())
        || !CURRENCIES.contains(&preferences.currency.as_str())
    {
        return Err("Unsupported regional preference".to_string());
    }
    Ok(preferences)
}

fn valid_time_zone(value: &str) -> bool {
    value == "system" || (value.len() <= 100 && value.parse::<chrono_tz::Tz>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_supported_preferences_and_rejects_unknown_values() {
        assert!(validate(default_value()).is_ok());
        let mut invalid = default_value();
        invalid["currency"] = json!("BTC");
        assert!(validate(invalid).is_err());
        assert!(valid_time_zone("Asia/Tokyo") && valid_time_zone("UTC"));
        assert!(!valid_time_zone("America//New_York"));
    }

    #[test]
    fn loads_the_saved_regional_preferences() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let preferences = load(&connection).expect("regional preferences load");
        assert_eq!(preferences.currency, "JPY");
    }
}
