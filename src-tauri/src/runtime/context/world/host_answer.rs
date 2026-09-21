//! Safe fallback and source selection for present-state questions.
use super::state_claim::StateClaim;
use saaa_personal_state_core::world::frame_sources::*;
pub(crate) fn matches_query(content: &str, kind: WorldSourceKind) -> bool {
    if content.contains("会議")
        || content.contains("ミーティング")
        || content.to_lowercase().contains("meeting")
    {
        return kind == WorldSourceKind::Situation;
    }
    if content.contains("期限")
        || content.contains("締切")
        || content.to_lowercase().contains("deadline")
    {
        return kind == WorldSourceKind::Schedule;
    }
    matches!(kind, WorldSourceKind::Coding | WorldSourceKind::Delegation)
}
pub(crate) fn card(state: &crate::AppState, run_id: &str, content: &str) -> String {
    let current = (|| -> Result<String, String> {
        let (service, frame) = super::app_frame::prepare(state, run_id)?;
        let claims: Vec<_> = frame
            .frame()
            .sources
            .iter()
            .filter(|g| g.availability == WorldSourceAvailability::Available)
            .flat_map(|g| &g.entries)
            .filter(|s| {
                matches_query(content, s.kind)
                    && s.availability == WorldSourceAvailability::Available
            })
            .filter_map(|s| {
                s.payload.clone().map(|value| StateClaim {
                    kind: s.kind,
                    source_ref: s.source_id.clone(),
                    source_version_or_digest: s.digest.clone(),
                    value,
                    as_of_ms: s.as_of_ms,
                })
            })
            .take(1)
            .collect();
        let raw = serde_json::json!({"claims":claims}).to_string();
        if service
            .validate_result(&frame)
            .map_err(|e| e.code().to_string())?
            != saaa_personal_state_core::world::runtime_frame::FrameValidity::Current
        {
            return Err("source-changed".into());
        }
        super::state_claim::render(&raw, frame.frame())
    })();
    current.unwrap_or_else(|_| {
        "この依頼の対象について、現在の状態を確認できる根拠がありません。状態は不明です。".into()
    })
}
