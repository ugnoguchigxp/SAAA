use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::load_model_providers;
use crate::providers::reachability::Reachability;
use crate::providers::service_harness::{HarnessResolution, HarnessServiceStatus};
use crate::{AppState, ModelProviderSettings};
use std::time::Duration;

pub(in crate::diagnosis) async fn harness(state: &AppState) -> Vec<DiagnosisItem> {
    let settings = match state.sqlite_readers.read(load_model_providers) {
        Ok(settings) => settings,
        Err(_) => return Vec::new(),
    };
    if settings.harness.address.trim().is_empty() {
        return vec![
            item(
                "harness.reachability",
                "harness",
                "Harness reachability",
                DiagnosisStatus::Skipped,
                DiagnosisSeverity::Degraded,
                "harness address is empty",
                None,
            ),
            item(
                "harness.resolve",
                "harness",
                "Harness resolve",
                DiagnosisStatus::Skipped,
                DiagnosisSeverity::Degraded,
                "harness address is empty",
                None,
            ),
        ];
    }
    let mut items = vec![reachability_item(state, &settings.providers)];
    match tokio::time::timeout(
        Duration::from_secs(10),
        crate::providers::service_harness::resolve_with_legacy_llm(&settings.harness.address),
    )
    .await
    {
        Ok(Ok(resolution)) => items.extend(items_from_resolution(&resolution)),
        Ok(Err(error)) => items.push(item(
            "harness.resolve",
            "harness",
            "Harness resolve",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            &error,
            None,
        )),
        Err(_) => items.push(item(
            "harness.resolve",
            "harness",
            "Harness resolve",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Degraded,
            "timed out after 10s",
            None,
        )),
    }
    items
}

fn reachability_item(state: &AppState, providers: &[ModelProviderSettings]) -> DiagnosisItem {
    let has_dynamic_lan = providers.iter().any(|provider| {
        provider.enabled() && matches!(provider, ModelProviderSettings::DynamicLan(_))
    });
    if !has_dynamic_lan {
        return item(
            "harness.reachability",
            "harness",
            "Harness reachability",
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Degraded,
            "no Dynamic LAN provider",
            None,
        );
    }
    let (status, message) = match state.reachability.snapshot().harness {
        Reachability::Reachable => (DiagnosisStatus::Ok, ""),
        Reachability::Unknown => (DiagnosisStatus::Warn, "reachability is not observed yet"),
        Reachability::Unreachable => (DiagnosisStatus::Fail, "harness is unreachable"),
    };
    item(
        "harness.reachability",
        "harness",
        "Harness reachability",
        status,
        DiagnosisSeverity::Degraded,
        message,
        None,
    )
}

pub(super) fn items_from_resolution(resolution: &HarnessResolution) -> Vec<DiagnosisItem> {
    let mut items = resolution
        .services
        .iter()
        .map(service_item)
        .collect::<Vec<_>>();
    if !resolution
        .services
        .iter()
        .any(|service| service.capability == "embedding")
    {
        items.push(item(
            "harness.embedding",
            "harness",
            "Harness embedding",
            DiagnosisStatus::Skipped,
            DiagnosisSeverity::Info,
            "embedding is not advertised",
            None,
        ));
    }
    items
}

fn service_item(service: &HarnessServiceStatus) -> DiagnosisItem {
    let embedding = service.capability == "embedding";
    let status = match service.state {
        "ready" => DiagnosisStatus::Ok,
        "degraded" => DiagnosisStatus::Warn,
        "unavailable" => DiagnosisStatus::Fail,
        _ => DiagnosisStatus::Warn,
    };
    item(
        &format!("harness.{}", service.capability),
        "harness",
        &format!("Harness {}", service.capability),
        status,
        if embedding {
            DiagnosisSeverity::Info
        } else {
            DiagnosisSeverity::Degraded
        },
        &service.message,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn write_address(state: &AppState, address: &str) {
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE settings_documents
                         SET value_json=json_set(value_json, '$.harness.address', ?1)
                         WHERE namespace='providers.model' AND key='default'",
                        [address],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("address update");
    }

    #[tokio::test]
    async fn dg_06_harness_skipped_when_address_empty() {
        let state = fresh();
        write_address(&state, "");
        let items = harness(&state).await;
        assert!(items.len() >= 2);
        assert!(items
            .iter()
            .all(|item| item.status == DiagnosisStatus::Skipped));
        assert!(items.iter().any(|item| item.id == "harness.reachability"));
        assert!(items.iter().any(|item| item.id == "harness.resolve"));
    }

    #[tokio::test]
    async fn dg_06_harness_resolve_error_yields_single_fail() {
        let state = fresh();
        write_address(&state, "http://127.0.0.1:9/");
        let items = harness(&state).await;
        let failures = items
            .iter()
            .filter(|item| item.status == DiagnosisStatus::Fail)
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].id, "harness.resolve");
        assert!(items.iter().all(|item| !item.id.starts_with("harness.llm")));
    }

    #[test]
    fn dg_06_embedding_maps_ready_and_missing() {
        let ready = items_from_resolution(&HarnessResolution {
            state: "ready",
            revision: "rev".into(),
            services: vec![HarnessServiceStatus {
                capability: "embedding",
                state: "ready",
                protocol: None,
                model: Some("embed".into()),
                language: None,
                voice: None,
                message: "ready".into(),
            }],
        });
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "harness.embedding");
        assert_eq!(ready[0].status, DiagnosisStatus::Ok);
        assert_eq!(ready[0].severity, DiagnosisSeverity::Info);

        let missing = items_from_resolution(&HarnessResolution {
            state: "degraded",
            revision: "rev".into(),
            services: vec![],
        });
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].status, DiagnosisStatus::Skipped);
        assert_eq!(missing[0].severity, DiagnosisSeverity::Info);
    }
}
