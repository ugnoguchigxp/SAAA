use super::ResolvedRoute;

pub(crate) fn validate_active(
    db: &rusqlite::Connection,
    route: &ResolvedRoute,
) -> Result<(), String> {
    let snapshot = crate::persistence::service_registry_store::load_registry(db)?.snapshot;
    let active = snapshot
        .binding(route.purpose)
        .is_some_and(|b| b.enabled && (route.location != "cloud" || b.cloud_allowed))
        && snapshot
            .resource(&route.resource_id)
            .is_some_and(|r| r.enabled && r.connection_id == route.connection_id)
        && snapshot
            .connection(&route.connection_id)
            .is_some_and(|c| c.enabled && c.credential_ref == route.credential_ref);
    let secret_present = match &route.credential_ref {
        Some(reference) => db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM credential_secrets WHERE service=?1 AND account=?2)",
                rusqlite::params![reference.service, reference.account],
                |r| r.get::<_, bool>(0),
            )
            .map_err(crate::database_error)?,
        None => true,
    };
    if active && secret_present {
        Ok(())
    } else {
        Err("実行中のサービスが無効化されたため、追加送信を停止しました。".into())
    }
}
