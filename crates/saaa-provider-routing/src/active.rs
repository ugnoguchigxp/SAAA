use rusqlite::{params, Connection};

use crate::{database_error, RegistrySnapshot, ResolvedRoute};

pub fn route_still_active(snapshot: &RegistrySnapshot, route: &ResolvedRoute) -> bool {
    snapshot.binding(route.purpose).is_some_and(|binding| {
        binding.enabled && (route.location != "cloud" || binding.cloud_allowed)
    }) && snapshot
        .resource(&route.resource_id)
        .is_some_and(|resource| resource.enabled && resource.connection_id == route.connection_id)
        && snapshot
            .connection(&route.connection_id)
            .is_some_and(|connection| {
                connection.enabled && connection.credential_ref == route.credential_ref
            })
}

/// Re-reads secret presence on the caller's transaction. The snapshot is the one
/// the host loaded on that same connection, including any legacy overlay.
pub fn validate_active(
    db: &Connection,
    route: &ResolvedRoute,
    snapshot: &RegistrySnapshot,
) -> Result<(), String> {
    let secret_present = match &route.credential_ref {
        Some(reference) => db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM credential_secrets WHERE service=?1 AND account=?2)",
                params![reference.service, reference.account],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?,
        None => true,
    };
    if route_still_active(snapshot, route) && secret_present {
        Ok(())
    } else {
        Err("実行中のサービスが無効化されたため、追加送信を停止しました。".into())
    }
}
