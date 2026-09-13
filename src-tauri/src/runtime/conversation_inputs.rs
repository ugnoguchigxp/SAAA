use crate::memory;
use crate::persistence::{
    load_codex_settings, load_model_providers, load_routing_settings, load_security_settings,
};
use crate::{database_error, AppState, StartTurnInput};
use rusqlite::params;

pub(super) struct Inputs {
    pub(super) providers: crate::ModelProvidersSettings,
    pub(super) route: crate::ConversationRouteSettings,
    pub(super) security: crate::SecurityRuntimeSettings,
    pub(super) identity: crate::CodexAgentRuntimeSettings,
    pub(super) regional: crate::persistence::settings::regional_preferences::RegionalPreferences,
    pub(super) loaded_context: memory::context_window::LoadedContextWindow,
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
        let loaded_context =
            memory::context_window::load(connection, &input.conversation_id, &input_message_id)?;
        let identity = load_codex_settings(connection)?;
        let regional = crate::persistence::settings::regional_preferences::load(connection)?;
        let providers = load_model_providers(connection)?;
        let route = load_routing_settings(connection)?.conversation_respond;
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
            configuration_fingerprint,
        })
    })
}
