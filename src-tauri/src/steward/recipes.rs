//! Host-owned test recipes. Models receive ids, never argv.
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecipeInput {
    pub name: String,
    pub target: String,
    pub cwd: String,
    pub argv: Vec<String>,
    pub env_allow: Vec<String>,
    pub output_dir: String,
    #[ts(type = "number")]
    pub timeout_ms: i64,
}

pub(crate) fn register(
    connection: &Connection,
    input: &RecipeInput,
) -> Result<serde_json::Value, String> {
    if input.argv.is_empty()
        || input
            .argv
            .iter()
            .any(|part| part.contains('`') || part.contains('$'))
    {
        return Err("recipe_invalid".into());
    }
    if input.network_forbidden() {
        return Err("recipe_invalid".into());
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}|{}|{}|{:?}|{}",
                input.target, input.cwd, input.output_dir, input.argv, input.timeout_ms
            )
            .as_bytes()
        )
    );
    let id = new_id("recipe");
    connection
        .execute(
            "INSERT INTO steward_recipes(id,revision,digest,name,target,cwd,argv_json,env_allow_json,output_dir,network,timeout_ms,created_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,0,?9,?10)",
            params![
                id,
                digest,
                input.name,
                input.target,
                input.cwd,
                serde_json::to_string(&input.argv).map_err(|_| "recipe_invalid")?,
                serde_json::to_string(&input.env_allow).map_err(|_| "recipe_invalid")?,
                input.output_dir,
                input.timeout_ms,
                now_iso()
            ],
        )
        .map_err(database_error)?;
    Ok(serde_json::json!({"recipeId": id, "revision": 1, "digest": digest}))
}

impl RecipeInput {
    fn network_forbidden(&self) -> bool {
        self.argv.iter().any(|part| {
            part.contains("curl") || part.contains("http://") || part.contains("https://")
        })
    }
}

pub(crate) fn load(
    connection: &Connection,
    recipe_id: &str,
    expected_digest: Option<&str>,
) -> Result<(Vec<String>, String, String, i64), String> {
    let (argv, cwd, output, digest, timeout): (String, String, String, String, i64) = connection
        .query_row(
            "SELECT argv_json,cwd,output_dir,digest,timeout_ms FROM steward_recipes WHERE id=?1",
            [recipe_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|_| "recipe_unknown".to_string())?;
    if expected_digest.is_some_and(|expected| expected != digest) {
        return Err("recipe_revision_changed".into());
    }
    let argv: Vec<String> = serde_json::from_str(&argv).map_err(|_| "recipe_invalid")?;
    Ok((argv, cwd, output, timeout))
}
