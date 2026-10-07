//! Desktop adapters. The service owns the run; this module only supplies the existing writer.
use crate::persistence::{SqliteReaders, SqliteWriter};
use crate::providers::reachability::{Reachability, ReachabilityState};
use rusqlite::Connection;
use saaa_media::{
    validate_larm_token, validate_named_secret, AvailabilitySource, CredentialSource, DbOwner,
    LiveBackend, MediaHostError, MediaService, MediaStore, SqlStore, SystemClock,
};
use saaa_provider_routing::LarmReachability;
use std::sync::Arc;
use zeroize::Zeroizing;

struct DesktopDb {
    writer: Arc<SqliteWriter>,
    readers: SqliteReaders,
}

impl DbOwner for DesktopDb {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.readers.read(operation)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.writer.write(operation)
    }
}

struct DesktopCredentials {
    writer: Arc<SqliteWriter>,
}

impl CredentialSource for DesktopCredentials {
    fn named_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<String>>, MediaHostError> {
        self.writer
            .read_serialized(|connection| {
                let value = saaa_provider_routing::read_named_secret(connection, service, account)?;
                if let Some(value) = &value {
                    validate_named_secret(service, account, value)
                        .map_err(|error| error.message)?;
                }
                Ok(value.map(Zeroizing::new))
            })
            .map_err(MediaHostError::storage)
    }

    fn larm_token(&self) -> Result<Zeroizing<String>, MediaHostError> {
        let loaded = crate::providers::dynamic_lan::credential::load().map_err(|error| {
            MediaHostError::new(error.code(), "LARMの資格情報を確認してください。")
        })?;
        validate_larm_token(loaded.token())?;
        Ok(Zeroizing::new(loaded.token().to_string()))
    }
}

struct DesktopAvailability {
    reachability: Arc<ReachabilityState>,
}

impl AvailabilitySource for DesktopAvailability {
    fn larm(&self) -> LarmReachability {
        match self.reachability.snapshot().harness {
            Reachability::Unknown => LarmReachability::Unknown,
            Reachability::Reachable => LarmReachability::Reachable,
            Reachability::Unreachable => LarmReachability::Unreachable,
        }
    }
}

pub(crate) fn assemble(
    writer: Arc<SqliteWriter>,
    readers: SqliteReaders,
    reachability: Arc<ReachabilityState>,
) -> MediaService {
    let credentials: Arc<dyn CredentialSource> = Arc::new(DesktopCredentials {
        writer: writer.clone(),
    });
    let store: Arc<dyn MediaStore> = Arc::new(SqlStore::new(
        DesktopDb { writer, readers },
        Arc::new(|connection| {
            crate::persistence::service_registry_store::load_registry(connection)
                .map(|loaded| loaded.snapshot)
        }),
    ));
    let backend = Arc::new(LiveBackend::new(store.clone(), credentials));
    MediaService::new(
        store,
        Arc::new(DesktopAvailability { reachability }),
        Arc::new(SystemClock),
        backend,
    )
}
