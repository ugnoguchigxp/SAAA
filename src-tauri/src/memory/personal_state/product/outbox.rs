//! Durable identity before dispatch, and recovery ownership even when a future is dropped.
use super::*;
pub struct Flight {
    pub writer: Arc<SqliteWriter>,
    pub generation: String,
}
impl Drop for Flight {
    fn drop(&mut self) {
        let _=self.writer.write(|c|{
            c.execute("UPDATE personal_generations SET status='interrupted',output_allowed=0,cancellation='requested' WHERE id=?1 AND status IN ('prepared','running')",[&self.generation]).map_err(database_error)?;
            c.execute("UPDATE personal_registrations SET pins=0,desired='deleted' WHERE incarnation IN (SELECT id FROM personal_remote_operations WHERE generation_id=?1 AND kind='source')",[&self.generation]).map_err(database_error)?;
            Ok(())
        });
    }
}
pub fn remember(
    a: &Adapter,
    id: &str,
    kind: &str,
    m: &Manifest,
    digest: &str,
) -> Result<(), String> {
    let cap = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    a.writer.write(|c|{
        generation::allow(c,&m.generation_id)?;
        c.execute("INSERT INTO personal_remote_operations(id,kind,generation_id,subject,allocation,runtime,request_digest,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",rusqlite::params![id,kind,m.generation_id,cap.subject_digest,cap.allocation_id,cap.runtime,digest,super::super::now()]).map_err(database_error)?;Ok(())
    })
}
pub fn receipt(a: &Adapter, id: &str, v: &Value) -> Result<(), String> {
    a.writer.write(|c| {
        c.execute(
            "UPDATE personal_remote_operations SET state='confirmed',request_digest=CASE WHEN EXISTS(SELECT 1 FROM personal_generations g WHERE g.id=generation_id AND g.output_allowed=0) THEN '' ELSE request_digest END,receipt=CASE WHEN EXISTS(SELECT 1 FROM personal_generations g WHERE g.id=generation_id AND g.output_allowed=0) THEN json_object('sourceHandle',json_extract(?2,'$.sourceHandle'),'state',json_extract(?2,'$.state'),'stopState',json_extract(?2,'$.stopState')) ELSE ?2 END WHERE id=?1",
            rusqlite::params![id, super::super::encode(v)?],
        )
        .map_err(database_error)?;
        Ok(())
    })
}
