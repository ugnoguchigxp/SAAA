use rusqlite::{params, Connection, OptionalExtension};

use super::{compile, validate, CompiledDictionary, Entry, ExpectedEntry};

pub(crate) const CONFLICT: &str = "辞書が変更されています。最新の登録を確認してください。";

pub(crate) struct Mutation {
    pub(crate) entries: Vec<Entry>,
    pub(crate) dictionary: Option<CompiledDictionary>,
    pub(crate) previous: Option<String>,
}

fn db(error: rusqlite::Error) -> String {
    format!("辞書データベースの処理に失敗しました: {error}")
}

pub(crate) fn lookup(connection: &Connection, written: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT spoken FROM tts_dictionary WHERE written=?1",
            [written],
            |row| row.get(0),
        )
        .optional()
        .map_err(db)
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<Entry>, String> {
    let mut statement = connection
        .prepare("SELECT written,spoken FROM tts_dictionary ORDER BY written")
        .map_err(db)?;
    let rows = statement
        .query_map([], |row| {
            Ok(Entry {
                written: row.get(0)?,
                spoken: row.get(1)?,
            })
        })
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

pub(crate) fn save(
    connection: &mut Connection,
    original: Option<&str>,
    entry: &Entry,
    expected: &ExpectedEntry,
    timestamp: &str,
    admit: impl Fn(&Connection) -> Result<(), String>,
) -> Result<Mutation, String> {
    validate(entry)?;
    let tx = connection.transaction().map_err(db)?;
    admit(&tx)?;
    let previous = match original {
        Some(written) => lookup(&tx, written)?,
        None => None,
    };
    // Idempotent retries must not rebuild the index or refresh updated_at.
    if original == Some(entry.written.as_str())
        && previous.as_deref() == Some(entry.spoken.as_str())
    {
        return Ok(Mutation {
            entries: list(&tx)?,
            dictionary: None,
            previous,
        });
    }
    if previous != expected.spoken {
        return Err(CONFLICT.into());
    }
    if original != Some(entry.written.as_str()) && lookup(&tx, &entry.written)?.is_some() {
        return Err(CONFLICT.into());
    }
    if let Some(written) = original.filter(|written| *written != entry.written) {
        tx.execute("DELETE FROM tts_dictionary WHERE written=?1", [written])
            .map_err(db)?;
    }
    tx.execute(
        "INSERT INTO tts_dictionary(written,spoken,updated_at) VALUES(?1,?2,?3)
        ON CONFLICT(written) DO UPDATE SET spoken=excluded.spoken,updated_at=excluded.updated_at",
        params![entry.written, entry.spoken, timestamp],
    )
    .map_err(db)?;
    let entries = list(&tx)?;
    let dictionary = compile(entries.clone());
    admit(&tx)?;
    tx.commit().map_err(db)?;
    Ok(Mutation {
        entries,
        dictionary: Some(dictionary),
        previous,
    })
}

pub(crate) fn delete(
    connection: &mut Connection,
    written: &str,
    expected: &ExpectedEntry,
) -> Result<Mutation, String> {
    let tx = connection.transaction().map_err(db)?;
    let previous = lookup(&tx, written)?;
    if previous.is_none() {
        return Ok(Mutation {
            entries: list(&tx)?,
            dictionary: None,
            previous,
        });
    }
    if previous != expected.spoken {
        return Err(CONFLICT.into());
    }
    tx.execute("DELETE FROM tts_dictionary WHERE written=?1", [written])
        .map_err(db)?;
    let entries = list(&tx)?;
    let dictionary = compile(entries.clone());
    tx.commit().map_err(db)?;
    Ok(Mutation {
        entries,
        dictionary: Some(dictionary),
        previous,
    })
}
