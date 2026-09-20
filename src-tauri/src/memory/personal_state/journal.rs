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
struct Document {
    version: u32,
    principal: String,
    tombstones: BTreeMap<String, i64>,
}
pub(crate) struct Journal {
    path: PathBuf,
    document: Document,
    dirty: bool,
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
                if d.version != 1 || d.principal != principal {
                    return Err("personal-forget-journal-identity".into());
                }
                d
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_create => Document {
                version: 1,
                principal,
                tombstones: BTreeMap::new(),
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
