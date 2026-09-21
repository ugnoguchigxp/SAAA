//! Deterministic, local-only dataset export. Manifests are published only after JSONL is durable.
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DatasetExport {
    pub(crate) dataset_id: String,
    pub(crate) examples_path: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) examples_digest: String,
    pub(crate) example_count: usize,
}

/// Exports ready examples in a stable order. Text-bearing fields are rejected rather than silently
/// copied into a learning artifact. Both files use a sibling `.tmp` and atomic rename; the
/// manifest is written last, so its presence is the publication signal.
pub(crate) fn export_ready_dataset(
    connection: &Connection,
    directory: &Path,
    dataset_id: &str,
) -> Result<DatasetExport, String> {
    let ready: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rr_datasets WHERE id=?1 AND state='ready')",
            [dataset_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !ready {
        return Err("Learning dataset is not ready".into());
    }
    let mut statement = connection
        .prepare("SELECT e.id,e.decision_id,e.feature_version,e.labeler_version,e.features_json,e.labels_json,e.eligible,e.exclusion_reason,
                         COALESCE((SELECT r.conversation_id FROM rr_example_sources s JOIN rr_roots r ON r.root_id=s.scope_key WHERE s.example_id=e.id AND s.source_kind='decision' LIMIT 1),
                                  (SELECT s.scope_key FROM rr_example_sources s WHERE s.example_id=e.id AND s.source_kind='decision' LIMIT 1), e.decision_id)
                  FROM rr_examples e WHERE e.dataset_id=?1 ORDER BY e.id")
        .map_err(|error| error.to_string())?;
    let examples = statement
        .query_map([dataset_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut jsonl = Vec::new();
    for (
        id,
        decision_id,
        feature_version,
        labeler_version,
        features,
        labels,
        eligible,
        exclusion_reason,
        group_key,
    ) in examples.iter()
    {
        let features: Value = serde_json::from_str(features)
            .map_err(|_| "Learning features are invalid".to_string())?;
        let labels: Value =
            serde_json::from_str(labels).map_err(|_| "Learning labels are invalid".to_string())?;
        if contains_textual_payload(&features) || contains_textual_payload(&labels) {
            return Err("Learning export refuses text-bearing fields".into());
        }
        serde_json::to_writer(&mut jsonl, &json!({"id":id,"decisionId":decision_id,"groupKey":group_key,"split":group_split(group_key),"featureVersion":feature_version,"labelerVersion":labeler_version,"features":features,"labels":labels,"eligible":eligible != &0,"exclusionReason":exclusion_reason}))
            .map_err(|error| error.to_string())?;
        jsonl.push(b'\n');
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let examples_path = directory.join(format!("{dataset_id}.jsonl"));
    let manifest_path = directory.join(format!("{dataset_id}.manifest.json"));
    atomic_write(&examples_path, &jsonl)?;
    let digest = sha256(&jsonl);
    let manifest = json!({"datasetId":dataset_id,"examplesFile":examples_path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),"examplesDigest":digest,"exampleCount":examples.len()});
    atomic_write(
        &manifest_path,
        serde_json::to_string(&manifest)
            .map_err(|error| error.to_string())?
            .as_bytes(),
    )?;
    Ok(DatasetExport {
        dataset_id: dataset_id.into(),
        examples_path,
        manifest_path,
        examples_digest: digest,
        example_count: examples.len(),
    })
}

fn group_split(group_key: &str) -> &'static str {
    let digest = Sha256::digest(group_key.as_bytes());
    if digest[0] % 5 == 0 {
        "eval"
    } else {
        "train"
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("data")
    ));
    let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn contains_textual_payload(value: &Value) -> bool {
    match value {
        Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "content" | "text" | "prompt" | "message" | "payload"
            ) || contains_textual_payload(value)
        }),
        Value::Array(values) => values.iter().any(contains_textual_payload),
        _ => false,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_33_partial_file_not_ready_and_export_is_deterministic() {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY,conversation_id TEXT); CREATE TABLE rr_decisions(id TEXT PRIMARY KEY); INSERT INTO rr_roots VALUES('r','conversation-1'); INSERT INTO rr_decisions VALUES('d');").expect("base");
        super::super::schema::migrate(&connection).expect("schema");
        connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('d',1,'f','l','{}','building',1)", []).expect("dataset");
        let directory = tempfile::tempdir().expect("directory");
        assert!(export_ready_dataset(&connection, directory.path(), "d").is_err());
        assert!(!directory.path().join("d.manifest.json").exists());
        connection
            .execute("UPDATE rr_datasets SET state='ready' WHERE id='d'", [])
            .expect("ready");
        connection.execute("INSERT INTO rr_examples(id,dataset_id,decision_id,label_revision,feature_version,labeler_version,features_json,labels_json,eligible,created_at_ms) VALUES('e','d','d',1,'f','l','{\"complexity\":2}','{\"outcome\":\"positive\"}',1,1)", []).expect("example");
        connection.execute("INSERT INTO rr_example_sources(example_id,source_kind,source_id,source_version,scope_key) VALUES('e','decision','d','1','r')", []).expect("source");
        let first = export_ready_dataset(&connection, directory.path(), "d").expect("export");
        let second =
            export_ready_dataset(&connection, directory.path(), "d").expect("export again");
        assert_eq!(first.examples_digest, second.examples_digest);
        assert!(first.examples_path.exists() && first.manifest_path.exists());
    }

    #[test]
    fn rr_33_group_split_keeps_one_conversation_in_one_partition() {
        assert_eq!(group_split("conversation-a"), group_split("conversation-a"));
        let groups = (0..100)
            .map(|index| group_split(&format!("conversation-{index}")))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(groups, std::collections::BTreeSet::from(["eval", "train"]));
    }
}
