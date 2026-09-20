//! Persistence for execution-time inspections (plan 12.4). A row exists only after both the
//! TypeScript projection and the report were written and renamed into place.

use rusqlite::{params, Connection, OptionalExtension};

use super::super::errors::*;

#[derive(Clone, Debug)]
pub struct NewInspection {
    pub id: String,
    pub revision_id: String,
    pub inspector_digest: String,
    pub package_hash: String,
    pub source_hash: String,
    pub program_hash: String,
    pub artifact_hash: String,
    pub projection_hash: String,
    pub relative_directory: String,
    pub comparison_json: String,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct InspectionRow {
    pub id: String,
    pub revision_id: String,
    pub inspector_digest: String,
    pub package_hash: String,
    pub source_hash: String,
    pub program_hash: String,
    pub artifact_hash: String,
    pub projection_hash: String,
    pub relative_directory: String,
    pub comparison_json: String,
    pub created_at: i64,
}

const COLUMNS: &str =
    "id, revision_id, inspector_digest, package_hash, source_hash, program_hash, \
     artifact_hash, projection_hash, relative_directory, comparison_json, created_at";

fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<InspectionRow> {
    Ok(InspectionRow {
        id: row.get(0)?,
        revision_id: row.get(1)?,
        inspector_digest: row.get(2)?,
        package_hash: row.get(3)?,
        source_hash: row.get(4)?,
        program_hash: row.get(5)?,
        artifact_hash: row.get(6)?,
        projection_hash: row.get(7)?,
        relative_directory: row.get(8)?,
        comparison_json: row.get(9)?,
        created_at: row.get(10)?,
    })
}

/// Inserts the completed inspection. A duplicate `(revision_id, inspector_digest)` is a conflict;
/// the caller returns the existing row instead of overwriting evidence.
pub fn insert(connection: &Connection, inspection: &NewInspection) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_inspections(
               id, revision_id, inspector_digest, package_hash, source_hash, program_hash,
               artifact_hash, projection_hash, relative_directory, comparison_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                inspection.id,
                inspection.revision_id,
                inspection.inspector_digest,
                inspection.package_hash,
                inspection.source_hash,
                inspection.program_hash,
                inspection.artifact_hash,
                inspection.projection_hash,
                inspection.relative_directory,
                inspection.comparison_json,
                inspection.created_at,
            ],
        )
        .map_err(|error| {
            let text = error.to_string();
            let code = if text.contains("UNIQUE") {
                CapabilityErrorCode::Conflict
            } else {
                CapabilityErrorCode::StorageError
            };
            CapabilityError::new(code, "could not record the inspection")
        })?;
    Ok(())
}

pub fn by_id(connection: &Connection, id: &str) -> CapabilityResult<Option<InspectionRow>> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM generated_capability_inspections WHERE id = ?1"),
            params![id],
            row_from,
        )
        .optional()
        .map_err(storage)
}

pub fn by_revision_and_digest(
    connection: &Connection,
    revision_id: &str,
    inspector_digest: &str,
) -> CapabilityResult<Option<InspectionRow>> {
    connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM generated_capability_inspections
                 WHERE revision_id = ?1 AND inspector_digest = ?2"
            ),
            params![revision_id, inspector_digest],
            row_from,
        )
        .optional()
        .map_err(storage)
}

/// The most recent successful inspection for a revision, used to keep a retired revision's
/// evidence readable.
pub fn latest_for_revision(
    connection: &Connection,
    revision_id: &str,
) -> CapabilityResult<Option<InspectionRow>> {
    connection
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM generated_capability_inspections
                 WHERE revision_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1"
            ),
            params![revision_id],
            row_from,
        )
        .optional()
        .map_err(storage)
}

/// Known inspection directories, used at startup to delete orphans left by an interrupted write.
pub fn all_directories(connection: &Connection) -> CapabilityResult<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT relative_directory FROM generated_capability_inspections")
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    let mut directories = Vec::new();
    for row in rows {
        directories.push(row.map_err(storage)?);
    }
    Ok(directories)
}

fn storage(error: rusqlite::Error) -> CapabilityError {
    CapabilityError::new(CapabilityErrorCode::StorageError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema::initialize_database;

    fn seed_revision(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO generated_capabilities(id, created_at, updated_at) VALUES('c','x','x')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO generated_capability_revisions(
                   id, capability_id, package_hash, inventory_hash, contract_hash, runtime_digest,
                   required_acceptance_hash, provenance_json, manifest_json, contract_json,
                   metadata_json, state, created_at)
                 VALUES('r','c','p','i','c','d','a','{}','{}','{}','{}','candidate','x')",
                [],
            )
            .unwrap();
    }

    fn sample(id: &str, digest: &str) -> NewInspection {
        NewInspection {
            id: id.into(),
            revision_id: "r".into(),
            inspector_digest: digest.into(),
            package_hash: "p".repeat(64),
            source_hash: "d".repeat(64),
            program_hash: "e".repeat(64),
            artifact_hash: "f".repeat(64),
            projection_hash: "2".repeat(64),
            relative_directory: format!("generated-inspections/{id}"),
            comparison_json: "{}".into(),
            created_at: 1,
        }
    }

    #[test]
    fn one_inspection_per_revision_and_inspector_digest() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        seed_revision(&connection);
        insert(&connection, &sample("i1", &"a".repeat(64))).unwrap();
        assert!(insert(&connection, &sample("i2", &"a".repeat(64))).is_err());
        // A different inspector digest on the same revision is a separate evidence record.
        insert(&connection, &sample("i3", &"b".repeat(64))).unwrap();
        let found = by_revision_and_digest(&connection, "r", &"b".repeat(64))
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "i3");
        assert_eq!(
            latest_for_revision(&connection, "r").unwrap().unwrap().id,
            "i3"
        );
        assert_eq!(all_directories(&connection).unwrap().len(), 2);
    }
}
