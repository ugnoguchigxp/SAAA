use crate::memory;
use crate::persistence::{
    load_codex_settings, load_model_providers, load_routing_settings, load_security_settings,
};
use crate::{database_error, AppState, StartTurnInput};
use rusqlite::params;

#[path = "conversation_inputs_roles.rs"]
mod conversation_inputs_roles;

pub(super) struct Inputs {
    pub(super) providers: crate::ModelProvidersSettings,
    pub(super) route: crate::ConversationRouteSettings,
    pub(super) security: crate::SecurityRuntimeSettings,
    pub(super) identity: crate::CodexAgentRuntimeSettings,
    pub(super) regional: crate::persistence::settings::regional_preferences::RegionalPreferences,
    pub(super) loaded_context: memory::context_window::LoadedContextWindow,
    pub(super) scope: crate::runtime::context::scope::ScopeSnapshot,
    pub(super) personal_candidates: Vec<crate::runtime::context::source::Candidate>,
    pub(super) personal_source_error: Option<String>,
    pub(super) configuration_fingerprint: String,
}

// Read identity, routing and source history from one consistent SQLite snapshot.
pub(super) fn load(state: &AppState, input: &StartTurnInput) -> Result<Inputs, String> {
    state.sqlite_readers.read(|connection| {
        let input_message_id: String = connection
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                params![input.run_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let scope = crate::runtime::context::scope::load(connection, &input.run_id)?;
        let loaded_context = memory::context_window::load(
            connection,
            &input.conversation_id,
            &input_message_id,
            &scope,
        )?;
        let (personal_candidates, personal_source_error) =
            if memory::control_plane::memory_enabled() && scope.status == "resolved" {
                match memory::personal_state::projection::context_candidates(
                    connection,
                    &scope,
                    &input_message_id,
                    64_000,
                ) {
                    Ok(candidates) => (candidates, None),
                    Err(error) => (Vec::new(), Some(error)),
                }
            } else {
                (Vec::new(), None)
            };
        let identity = load_codex_settings(connection)?;
        let regional = crate::persistence::settings::regional_preferences::load(connection)?;
        let providers = load_model_providers(connection)?;
        let mut route = load_routing_settings(connection)?.conversation_respond;
        conversation_inputs_roles::apply_enabled_role_route(connection, &mut route)?;
        let configuration_fingerprint =
            crate::persistence::effective_route::conversation_configuration_fingerprint(
                &providers, &route,
            )?;
        Ok(Inputs {
            providers,
            route,
            security: load_security_settings(connection)?,
            identity,
            regional,
            loaded_context,
            scope,
            personal_candidates,
            personal_source_error,
            configuration_fingerprint,
        })
    })
}
