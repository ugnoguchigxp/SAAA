//! Minimal forget journal outside DB backups. Contains opaque IDs/timestamps only.
//! A restored v17 DB cannot become usable if its current journal is missing.
use crate::database_error;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    pub(super) version: u32,
    pub(super) principal: String,
    pub(super) tombstones: BTreeMap<String, i64>,
    #[serde(default)]
    pub(super) records: BTreeMap<String, i64>,
}
pub(crate) struct Journal {
    pub(super) path: PathBuf,
    pub(super) document: Document,
    pub(super) dirty: bool,
}
impl Journal {
    pub(crate) fn open(c: &Connection, path: PathBuf, allow_create: bool) -> Result<Self, String> {
        let principal: String = c
            .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
            .map_err(database_error)?;
        let document = match std::fs::read(&path) {
            Ok(bytes) => {
                let d: Document = serde_json::from_slice(&bytes)
                    .map_err(|_| "personal-forget-journal-corrupt")?;
                if (d.version != 1 && d.version != 2) || d.principal != principal {
                    return Err("personal-forget-journal-identity".into());
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_create => Document {
                version: 2,
                principal,
                tombstones: BTreeMap::new(),
                records: BTreeMap::new(),
            },
            Err(_) => return Err("personal-forget-journal-required".into()),
        };
        let mut journal = Self {
            path,
            document,
            dirty: false,
        };
        let tx = c.unchecked_transaction().map_err(database_error)?;
        let known = journal
            .document
            .tombstones
            .iter()
            .map(|(id, at)| (id.clone(), *at))
            .collect::<Vec<_>>();
        super::store::recover(&tx, Some(&known))?;
        crate::schedule::forget::recover(&tx, &known)?;
        for (id, at) in &journal.document.records {
            tx.execute(
                "INSERT OR IGNORE INTO record_tombstones(record_id, forgotten_at, forget_epoch, reason_code) VALUES(?1,?2,?2,'journal')",
                rusqlite::params![id, at],
            )
            .map_err(database_error)?;
        }
        tx.commit().map_err(database_error)?;
        journal.sync(c)?;
        if !journal.path.exists() {
            journal.persist()?;
        }
        Ok(journal)
    }
    pub(crate) fn sync(&mut self, c: &Connection) -> Result<(), String> {
        let count: usize = c
            .query_row("SELECT count(*) FROM personal_tombstones", [], |r| r.get(0))
            .map_err(database_error)?;
        let extra: usize = c
            .query_row("SELECT count(*) FROM schedule_tombstones", [], |r| r.get(0))
            .unwrap_or(0);
        if count + extra == self.document.tombstones.len() && !self.dirty {
            return Ok(());
        }
        let mut stmt = c
            .prepare("SELECT source_id,forgotten_at FROM personal_tombstones UNION ALL SELECT id,forgotten_at FROM schedule_tombstones")
            .map_err(database_error)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(database_error)?;
        let rows = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(stmt);
        for (id, at) in rows {
            self.document.tombstones.insert(id, at);
        }
        self.dirty = true;
        self.persist()?;
        self.dirty = false;
        Ok(())
    }
    fn persist(&self) -> Result<(), String> {
        let parent = self.path.parent().ok_or("personal-forget-journal-path")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "personal-forget-journal-write")?;
        let bytes =
            serde_json::to_vec(&self.document).map_err(|_| "personal-forget-journal-encode")?;
        temp.write_all(&bytes)
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| "personal-forget-journal-write")?;
        temp.persist(&self.path)
            .map_err(|_| "personal-forget-journal-write")?;
        std::fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| "personal-forget-journal-sync")?;
        Ok(())
    }
}
pub(crate) fn path(database: &Path) -> PathBuf {
    database.with_extension("forget.json")
}

pub(crate) fn sync_locked(
    journal: &Option<std::sync::Mutex<Journal>>,
    c: &Connection,
) -> Result<(), String> {
    if let Some(journal) = journal {
        journal
            .lock()
            .map_err(|_| "personal-forget-journal-unavailable")?
            .sync(c)?;
    }
    Ok(())
}

pub(crate) fn database_version(c: &Connection) -> rusqlite::Result<i64> {
    c.pragma_query_value(None, "user_version", |r| r.get(0))
}

pub(crate) fn open_database(
    c: &Connection,
    database: &Path,
    previous_version: i64,
) -> Result<Journal, String> {
    Journal::open(c, path(database), previous_version < 17)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_51_journal_recover_restores_record_tombstones() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let principal: String = connection
            .query_row("SELECT principal FROM personal_scope", [], |row| row.get(0))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.forget.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "principal": principal,
                "tombstones": {},
                "records": {"record-forgotten": 42}
            })
            .to_string(),
        )
        .unwrap();
        Journal::open(&connection, path, false).unwrap();
        let forgotten_at: i64 = connection
            .query_row(
                "SELECT forgotten_at FROM record_tombstones WHERE record_id='record-forgotten'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(forgotten_at, 42);
    }
}
