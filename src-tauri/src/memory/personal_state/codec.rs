pub fn now() -> i64 {
    crate::now_iso().parse().unwrap_or(0)
}
pub fn encode<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "personal-state-encoding".into())
}
pub fn decode<T: serde::de::DeserializeOwned>(value: String) -> Result<T, String> {
    serde_json::from_str(&value).map_err(|_| "personal-state-corrupt".into())
}
