use crate::persistence::load_role_routing_settings;
use crate::ConversationRouteSettings;
pub(super) use rusqlite::{Connection, OptionalExtension};
#[path = "conversation_inputs_roles/role_dispatch.rs"]
mod role_dispatch;
pub(crate) use role_dispatch::{RoleDispatch, CodexStepBinding};
pub(crate) use role_dispatch::apply_enabled_role_route;
#[cfg(test)]
pub(super) use crate::initialize_database;
#[cfg(test)]
pub(super) use crate::DYNAMIC_LAN_PROVIDER_ID;
#[cfg(test)]
pub(super) use crate::persistence::{
    list_settings_documents, save_settings_documents_to_connection,
};
#[cfg(test)]
pub(super) use crate::test_support::default_settings_input;
#[cfg(test)]
#[path = "conversation_inputs_roles/tests.rs"]
mod tests;
