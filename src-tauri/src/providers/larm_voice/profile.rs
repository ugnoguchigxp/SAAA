/// Not a valid profile id, so an operator profile literally named `auto` stays explicit.
pub(crate) const AUTO_LABEL: &str = "@auto";

pub(crate) fn preference(stored: Option<&str>) -> saaa_larm_session::ProfilePreference {
    match stored {
        None
        | Some(saaa_larm_session::PREVIOUS_DEFAULT_PROFILE)
        | Some(saaa_larm_session::CANONICAL_PROFILE) => saaa_larm_session::ProfilePreference::Auto,
        Some(value) => saaa_larm_session::ProfilePreference::Explicit(value.to_string()),
    }
}

pub(crate) fn label(preference: &saaa_larm_session::ProfilePreference) -> String {
    match preference {
        saaa_larm_session::ProfilePreference::Auto => AUTO_LABEL.into(),
        saaa_larm_session::ProfilePreference::Explicit(value) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::preference;

    #[test]
    fn preference_auto_for_shipped_values() {
        for stored in [
            None,
            Some("saaa-conversation-gemma4"),
            Some("saaa-conversation-ornith15"),
        ] {
            assert_eq!(
                preference(stored),
                saaa_larm_session::ProfilePreference::Auto
            );
        }
    }

    #[test]
    fn preference_respects_operator_choice() {
        assert_eq!(
            preference(Some("custom-x")),
            saaa_larm_session::ProfilePreference::Explicit("custom-x".into())
        );
        assert_eq!(
            preference(Some("auto")),
            saaa_larm_session::ProfilePreference::Explicit("auto".into())
        );
    }
}
