//! Safe fallback and source selection for present-state questions.
#[path = "host_answer_query.rs"]
mod query;
use super::state_claim::StateClaim;
pub(crate) use query::matches_query;
use saaa_personal_state_core::world::frame_sources::*;

pub(crate) struct PreparedCard {
    pub(crate) text: String,
    pub(crate) world: Option<super::app_frame::Prepared>,
}

pub(crate) fn prepare_card(state: &crate::AppState, run_id: &str, content: &str) -> PreparedCard {
    prepare_current_card(state, run_id, content).unwrap_or_else(|_| PreparedCard {
        text: "この依頼の対象について、現在の状態を確認できる根拠がありません。状態は不明です。"
            .into(),
        world: None,
    })
}

fn prepare_current_card(
    state: &crate::AppState,
    run_id: &str,
    content: &str,
) -> Result<PreparedCard, String> {
    let (service, frame) = super::app_frame::prepare(state, run_id)?;
    let claims: Vec<_> = frame
        .frame()
        .sources
        .iter()
        .filter(|g| g.availability == WorldSourceAvailability::Available)
        .flat_map(|g| &g.entries)
        .filter(|s| {
            matches_query(content, s.kind) && s.availability == WorldSourceAvailability::Available
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
    let text = super::state_claim::render_for_query(&raw, frame.frame(), content)?;
    Ok(PreparedCard {
        text,
        world: Some((service, frame)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_state_subjects_do_not_cross_source_kinds() {
        assert!(crate::runtime::context::state_answer::is_state_query(
            "現在のコーディングタスクは？"
        ));
        assert!(matches_query(
            "委任タスクの状態は？",
            WorldSourceKind::Delegation
        ));
        assert!(!matches_query(
            "委任タスクの状態は？",
            WorldSourceKind::Coding
        ));
        assert!(matches_query(
            "現在のコーディングタスクは？",
            WorldSourceKind::Coding
        ));
        assert!(!matches_query(
            "現在のコーディングタスクは？",
            WorldSourceKind::Delegation
        ));
        assert!(matches_query("次の期限は？", WorldSourceKind::Schedule));
        assert!(!matches_query("次の期限は？", WorldSourceKind::Situation));
    }

    #[test]
    fn prepared_card_retains_a_db_validation_fence() {
        use crate::memory::personal_state::world::runtime_test_support::{
            Fixture, CODING_ID, RUN_ID,
        };
        use std::sync::Arc;

        let fixture = Fixture::new(&[("task", CODING_ID)]);
        fixture.add_coding_job(1, "running", "running", "accepted");
        let capabilities = Arc::new(
            crate::generated_capabilities::service::CapabilityService::build(
                fixture.writer.clone(),
                &std::path::PathBuf::new(),
                std::path::PathBuf::new(),
                None,
            ),
        );
        let state =
            crate::test_state::app_state_with_capabilities(fixture.writer.clone(), capabilities);
        let prepared =
            prepare_current_card(&state, RUN_ID, "現在のコーディングタスクは？").unwrap();
        let (service, frame) = prepared.world.expect("source-backed card");
        fixture
            .writer
            .write(|connection| {
                connection
                    .execute("UPDATE personal_scope SET revision=revision+1", [])
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        assert!(fixture
            .writer
            .read_serialized(|connection| service
                .validate_db_result(connection, &frame)
                .map_err(|error| error.code().to_string()))
            .is_err());
    }
}
