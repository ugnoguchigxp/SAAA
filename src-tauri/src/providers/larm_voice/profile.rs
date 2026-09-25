pub(crate) fn preference(stored: Option<&str>) -> saaa_larm_session::ProfilePreference {
    let Some(value) = stored else {
        return saaa_larm_session::ProfilePreference::Variant(
            saaa_larm_session::ProfileVariant::Conversation,
        );
    };
    if saaa_larm_session::LEGACY_PROFILE_IDS.contains(&value) {
        return saaa_larm_session::ProfilePreference::Variant(
            saaa_larm_session::ProfileVariant::Conversation,
        );
    }
    match saaa_larm_session::ProfileVariant::from_selector(value) {
        Some(variant) => saaa_larm_session::ProfilePreference::Variant(variant),
        None => saaa_larm_session::ProfilePreference::Explicit(value.to_string()),
    }
}

pub(crate) fn label(preference: &saaa_larm_session::ProfilePreference) -> String {
    match preference {
        saaa_larm_session::ProfilePreference::Variant(variant) => {
            format!("@selector:{}", variant.selector())
        }
        saaa_larm_session::ProfilePreference::Explicit(value) => value.clone(),
    }
}

pub(crate) fn from_label(label: &str) -> saaa_larm_session::ProfilePreference {
    match label
        .strip_prefix("@selector:")
        .and_then(saaa_larm_session::ProfileVariant::from_selector)
    {
        Some(variant) => saaa_larm_session::ProfilePreference::Variant(variant),
        None => saaa_larm_session::ProfilePreference::Explicit(label.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{from_label, label, preference};

    fn conversation() -> saaa_larm_session::ProfilePreference {
        saaa_larm_session::ProfilePreference::Variant(
            saaa_larm_session::ProfileVariant::Conversation,
        )
    }

    #[test]
    fn preference_maps_shipped_values_to_saaa_selector() {
        for stored in [
            None,
            Some("SAAA"),
            Some("saaa-conversation-ornith15"),
            Some("saaa-conversation-gemma4"),
            Some("saaa-qwen38"),
        ] {
            assert_eq!(preference(stored), conversation());
        }
    }

    #[test]
    fn preference_maps_media_selectors() {
        assert_eq!(
            preference(Some("SAAA-w-Image")),
            saaa_larm_session::ProfilePreference::Variant(saaa_larm_session::ProfileVariant::Image)
        );
        assert_eq!(
            preference(Some("SAAA-w-music")),
            saaa_larm_session::ProfilePreference::Variant(saaa_larm_session::ProfileVariant::Music)
        );
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

    #[test]
    fn label_round_trips_every_preference() {
        let preferences = [
            conversation(),
            saaa_larm_session::ProfilePreference::Variant(saaa_larm_session::ProfileVariant::Image),
            saaa_larm_session::ProfilePreference::Variant(saaa_larm_session::ProfileVariant::Music),
            saaa_larm_session::ProfilePreference::Explicit("custom-x".into()),
            saaa_larm_session::ProfilePreference::Explicit("auto".into()),
        ];
        for preference in preferences {
            assert_eq!(from_label(&label(&preference)), preference);
        }
    }
}
