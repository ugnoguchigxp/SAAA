//! Common source-backed frame preparation for provider adapters outside the chat broker.
use crate::{
    memory::personal_state::world::runtime_frame::{
        FrameRequest, PreparedWorldFrame, WorldFrameService,
    },
    AppState,
};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::sync::Arc;

pub(crate) type Prepared = (Arc<WorldFrameService>, PreparedWorldFrame);

pub(crate) fn prepare(state: &AppState, run_id: &str) -> Result<Prepared, String> {
    let (scope, principal, policy) = state.sqlite_readers.read(|c| {
        let scope = crate::runtime::context::scope::load(c, run_id)?;
        let (principal, policy): (String, u64) = c
            .query_row(
                "SELECT principal,policy_revision FROM personal_scope WHERE id='primary'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(crate::database_error)?;
        Ok((scope, principal, policy))
    })?;
    let root = if scope.is_user_only() {
        format!("user:{principal}")
    } else {
        super::turn::explicit_project(&scope).map_err(|e| e.as_str().to_string())?
    };
    let service = Arc::new(
        WorldFrameService::new(
            state.sqlite_readers.clone(),
            Arc::new(crate::memory::personal_state::now),
        )
        .with_sources(state.situation.clone()),
    );
    let frame = service
        .prepare_frame(FrameRequest {
            run_id,
            project_scope: &root,
            access: AccessRequest {
                principal: &principal,
                scope: "primary",
                task_request: Some(&root),
                purpose: Purpose::Reasoning,
                max_classification: Classification::Confidential,
                policy_revision: policy,
                authorized: true,
            },
            runtime_refs: super::turn::runtime_refs(&scope),
            graph_request: if scope.is_user_only() {
                None
            } else {
                super::question_input::read(state, run_id).graph_request()
            },
            max_bytes: 8192,
            ttl_ms: 1000,
        })
        .map_err(|e| e.code().to_string())?;
    Ok((service, frame))
}
