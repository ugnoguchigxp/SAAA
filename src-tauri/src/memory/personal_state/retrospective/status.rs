use crate::database_error;
use rusqlite::Connection;
use serde_json::{json, Value};

pub(crate) fn status(c: &Connection) -> Result<Value, String> {
    let mode: String = c
        .query_row("SELECT mode FROM personal_review_settings", [], |r| {
            r.get(0)
        })
        .map_err(database_error)?;
    let mut stmt=c.prepare("SELECT status,result,count(*),min(updated_at) FROM personal_review_work GROUP BY status,result").map_err(database_error)?;
    let rows=stmt.query_map([], |r|Ok(json!({"stage":r.get::<_,String>(0)?,"reason":r.get::<_,String>(1)?,"count":r.get::<_,u64>(2)?,"updatedAt":r.get::<_,i64>(3)?}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    Ok(
        json!({"mode":mode,"stages":rows,"modelContextTarget":64000,"externalEvidence":"contract-unverified"}),
    )
}
