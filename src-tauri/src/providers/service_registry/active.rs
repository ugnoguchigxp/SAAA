use super::ResolvedRoute;

pub(crate) fn validate_active(
    db: &rusqlite::Connection,
    route: &ResolvedRoute,
) -> Result<(), String> {
    let snapshot = crate::persistence::service_registry_store::load_registry(db)?.snapshot;
    saaa_provider_routing::validate_active(db, route, &snapshot)
}
