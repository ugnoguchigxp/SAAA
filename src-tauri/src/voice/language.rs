use std::collections::HashSet;

pub(crate) const DEFAULT_LANGUAGE_CODE: &str = "ja";

const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[
    ("zh", "Chinese"),
    ("en", "English"),
    ("yue", "Cantonese"),
    ("ar", "Arabic"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ko", "Korean"),
    ("ru", "Russian"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("ja", "Japanese"),
    ("tr", "Turkish"),
    ("hi", "Hindi"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("fil", "Filipino"),
    ("fa", "Persian"),
    ("el", "Greek"),
    ("ro", "Romanian"),
    ("hu", "Hungarian"),
    ("mk", "Macedonian"),
];

pub(crate) fn default_allowed_languages() -> Vec<String> {
    vec![DEFAULT_LANGUAGE_CODE.to_string()]
}

pub(crate) fn is_supported_language_code(code: &str) -> bool {
    SUPPORTED_LANGUAGES
        .iter()
        .any(|(supported, _)| *supported == code)
}

pub(crate) fn validate_allowed_languages(languages: &[String]) -> Result<(), String> {
    if languages.is_empty() || languages.len() > SUPPORTED_LANGUAGES.len() {
        return Err("At least one supported ASR language must be registered".to_string());
    }
    let mut unique = HashSet::with_capacity(languages.len());
    if languages
        .iter()
        .any(|code| !is_supported_language_code(code) || !unique.insert(code.as_str()))
    {
        return Err("Registered ASR languages must be supported and unique".to_string());
    }
    Ok(())
}

fn canonical_code(language: &str) -> Option<&'static str> {
    let normalized = language.trim();
    SUPPORTED_LANGUAGES.iter().find_map(|(code, name)| {
        (normalized.eq_ignore_ascii_case(code) || normalized.eq_ignore_ascii_case(name))
            .then_some(*code)
    })
}

pub(crate) fn enforce_allowed_language(
    detected: Option<&str>,
    allowed: &[String],
) -> Result<(), String> {
    validate_allowed_languages(allowed)?;
    let detected = detected
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .ok_or_else(|| {
            "ASR_LANGUAGE_UNKNOWN: The ASR service did not return a detected language".to_string()
        })?;
    let mut codes = Vec::new();
    for language in detected.split(',') {
        let code = canonical_code(language).ok_or_else(|| {
            "ASR_LANGUAGE_UNKNOWN: The ASR service returned an unsupported detected language"
                .to_string()
        })?;
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        return Err(
            "ASR_LANGUAGE_UNKNOWN: The ASR service did not return a detected language".to_string(),
        );
    }
    if codes
        .iter()
        .any(|code| !allowed.iter().any(|allowed| allowed == code))
    {
        return Err(
            "ASR_LANGUAGE_NOT_ALLOWED: The detected language is not registered in Voice settings"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_registered_language_names_and_codes() {
        let allowed = vec!["ja".to_string(), "en".to_string()];
        assert!(enforce_allowed_language(Some("Japanese"), &allowed).is_ok());
        assert!(enforce_allowed_language(Some("ja"), &allowed).is_ok());
        assert!(enforce_allowed_language(Some("Japanese,English"), &allowed).is_ok());
    }

    #[test]
    fn rejects_unregistered_unknown_and_partial_language_matches() {
        let allowed = vec!["ja".to_string()];
        assert!(enforce_allowed_language(Some("Chinese"), &allowed)
            .expect_err("Chinese is rejected")
            .starts_with("ASR_LANGUAGE_NOT_ALLOWED"));
        assert!(enforce_allowed_language(Some("Japanese,English"), &allowed)
            .expect_err("partially registered speech is rejected")
            .starts_with("ASR_LANGUAGE_NOT_ALLOWED"));
        assert!(enforce_allowed_language(None, &allowed)
            .expect_err("missing language is rejected")
            .starts_with("ASR_LANGUAGE_UNKNOWN"));
        assert!(enforce_allowed_language(Some("Klingon"), &allowed)
            .expect_err("unknown language is rejected")
            .starts_with("ASR_LANGUAGE_UNKNOWN"));
    }

    #[test]
    fn registered_languages_must_be_non_empty_supported_and_unique() {
        assert!(validate_allowed_languages(&[]).is_err());
        assert!(validate_allowed_languages(&["xx".to_string()]).is_err());
        assert!(validate_allowed_languages(&["ja".to_string(), "ja".to_string()]).is_err());
    }
}
