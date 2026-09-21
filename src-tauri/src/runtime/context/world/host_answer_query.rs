use saaa_personal_state_core::world::frame_sources::WorldSourceKind;

pub(crate) fn matches_query(content: &str, kind: WorldSourceKind) -> bool {
    let lower = content.to_lowercase();
    if content.contains("会議") || content.contains("ミーティング") || lower.contains("meeting")
    {
        return kind == WorldSourceKind::Situation;
    }
    if content.contains("期限") || content.contains("締切") || lower.contains("deadline") {
        return kind == WorldSourceKind::Schedule;
    }
    if content.contains("委任") || lower.contains("delegat") {
        return kind == WorldSourceKind::Delegation;
    }
    if content.contains("実装")
        || content.contains("コーディング")
        || lower.contains("coding")
        || lower.contains("code task")
    {
        return kind == WorldSourceKind::Coding;
    }
    matches!(kind, WorldSourceKind::Coding | WorldSourceKind::Delegation)
}
