use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::load_model_providers;
use crate::AppState;

pub(in crate::diagnosis) fn settings(state: &AppState) -> DiagnosisItem {
    match state
        .sqlite_readers
        .read(load_model_providers)
    {
        Ok(settings) if settings.providers.is_empty() => item(
            "settings.providers",
            "settings",
            "Model providers",
            DiagnosisStatus::Warn,
            DiagnosisSeverity::Fatal,
            "No model providers are configured",
            None,
        ),
        Ok(_) => item(
            "settings.providers",
            "settings",
            "Model providers",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Fatal,
            "",
            None,
        ),
        Err(error) => item(
            "settings.providers",
            "settings",
            "Model providers",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Fatal,
            &error,
            None,
        ),
    }
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

    fn write_providers(state: &AppState, providers_json: &str) {
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE settings_documents
                         SET value_json=json_set(value_json, '$.providers', json(?1))
                         WHERE namespace='providers.model' AND key='default'",
                        [providers_json],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("providers update");
    }

    #[test]
    fn dg_04_settings_ok_with_defaults() {
        let item = settings(&fresh());
        assert_eq!(item.id, "settings.providers");
        assert_eq!(item.status, DiagnosisStatus::Ok);
        assert_eq!(item.severity, DiagnosisSeverity::Fatal);
    }

    #[test]
    fn dg_04_settings_warn_when_no_providers() {
        let state = fresh();
        write_providers(&state, "[]");
        let item = settings(&state);
        assert_eq!(item.status, DiagnosisStatus::Warn);
        assert_eq!(item.severity, DiagnosisSeverity::Fatal);
    }
}
