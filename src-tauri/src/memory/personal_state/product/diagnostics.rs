//! Owner diagnostics keep only phase names and states, never source/credential payloads.
use super::*;
use rusqlite::Connection;
use serde_json::json;
pub fn record(a: &Adapter, id: &str, v: &Value) -> Result<(), String> {
    let cap = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    if v["forgetId"] != id || v["subjectDigest"] != cap.subject_digest {
        return Err("personal-forget-binding".into());
    }
    let mut phases = serde_json::Map::new();
    for name in [
        "attempts",
        "views",
        "runtime",
        "snapshots",
        "registry",
        "sources",
        "audit",
    ] {
        let state = v["phases"][name]["state"]
            .as_str()
            .filter(|s| {
                matches!(
                    *s,
                    "pending" | "running" | "absent" | "failed" | "stop_unknown"
                )
            })
            .unwrap_or("pending");
        phases.insert(name.into(), json!(state));
    }
    let result = json!({"complete":saaa_larm_session::personal_state::forget_complete(v,&cap.subject_digest,id),"phases":phases});
    a.writer.write(|c|{c.execute("INSERT INTO personal_remote_results VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET result=excluded.result,updated_at=excluded.updated_at",rusqlite::params![id,result.to_string(),super::super::now()]).map_err(database_error)?;Ok(())})
}
pub fn read(c: &Connection) -> Result<Value, String> {
    let mut q = c
        .prepare("SELECT id,result FROM personal_remote_results ORDER BY updated_at DESC LIMIT 20")
        .map_err(database_error)?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut items = Vec::new();
    for (id, raw) in rows {
        let mut v: Value = super::super::decode(raw)?;
        v["id"] = json!(id);
        items.push(v);
    }
    Ok(json!(items))
}
