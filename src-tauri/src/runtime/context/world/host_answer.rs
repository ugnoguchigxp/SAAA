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
    match super::query_understand::understand(content) {
        super::query_understand::QueryUnderstanding::Ambiguous { candidates, reason } => {
            let names = if candidates.is_empty() {
                reason
            } else {
                candidates.join(" / ")
            };
            return Ok(PreparedCard {
                text: format!("どれを指していますか: {names}"),
                world: None,
            });
        }
        super::query_understand::QueryUnderstanding::Unavailable { reason } => {
            let _ = reason;
        }
        _ => {}
    }
    let (service, frame) = super::app_frame::prepare(state, run_id)?;
    let claims = claims_for_query(frame.frame(), content);
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

fn claims_for_query(
    frame: &saaa_personal_state_core::world::runtime_frame::WorldFrame,
    content: &str,
) -> Vec<StateClaim> {
    let mut candidates: Vec<_> = frame
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
        .collect();
    candidates.sort_by(|a, b| a.source_ref.cmp(&b.source_ref));
    let lower = content.to_lowercase();
    let explicit: Vec<_> = candidates
        .iter()
        .filter(|claim| {
            claim.source_ref.len() >= 3 && lower.contains(&claim.source_ref.to_lowercase())
        })
        .cloned()
        .collect();
    if explicit.len() == 1 {
        return explicit;
    }
    if candidates.len() == 1 {
        return candidates;
    }
    if candidates
        .first()
        .is_some_and(|claim| claim.kind == WorldSourceKind::Schedule)
    {
        candidates.sort_by(|a, b| {
            let due = |claim: &StateClaim| match &claim.value {
                WorldSourcePayload::Schedule { due_at_ms, .. } => due_at_ms.unwrap_or(i64::MAX),
                _ => i64::MAX,
            };
            due(a).cmp(&due(b)).then(a.source_ref.cmp(&b.source_ref))
        });
        return candidates.into_iter().take(1).collect();
    }
    // Multiple current/focus tasks cannot be ranked from the source payload
    // alone. Returning no claim produces the verified "unknown" fallback
    // instead of answering for an arbitrary task.
    Vec::new()
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

    #[test]
    fn host_fallback_never_picks_an_arbitrary_task() {
        use saaa_personal_state_core::world::frame_sources::{
            WorldScope, WorldSourceEntry, WorldSourceGroup,
        };
        let mut frame = saaa_personal_state_core::world::runtime_frame::WorldFrame::for_scope(
            "run",
            WorldScope {
                focus_scope_key: Some("project:p".into()),
                allowed_scope_keys: vec![
                    "project:p".into(),
                    "task:job-a".into(),
                    "task:job-b".into(),
                ],
                digest: "scope".into(),
            },
            1,
            2,
        )
        .unwrap();
        let entry = |id: &str| WorldSourceEntry {
            kind: WorldSourceKind::Coding,
            source_id: id.into(),
            owner_scope_key: format!("task:{id}"),
            availability: WorldSourceAvailability::Available,
            observed_at_ms: 1,
            as_of_ms: 1,
            version: Some("1".into()),
            digest: format!("digest-{id}"),
            payload: Some(WorldSourcePayload::Coding {
                job_id: id.into(),
                owner_state: "running".into(),
                phase: "running".into(),
                revision: Some(1),
            }),
            reason_code: None,
        };
        frame.sources.push(WorldSourceGroup {
            kind: WorldSourceKind::Coding,
            availability: WorldSourceAvailability::Available,
            entries: vec![entry("job-a"), entry("job-b")],
            omission_reason: None,
        });
        assert!(claims_for_query(&frame, "現在のコーディングタスクは？").is_empty());
        let selected = claims_for_query(&frame, "job-b の現在のコーディング状態は？");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source_ref, "job-b");
    }
}
