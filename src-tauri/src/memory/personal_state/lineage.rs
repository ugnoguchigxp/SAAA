//! Conservative input ancestry for existing tools whose result lacks exact ranges.
use crate::database_error;
use rusqlite::Connection;
use saaa_personal_state_core::SourceRef;
pub fn extend(c: &Connection, refs: &mut Vec<SourceRef>) -> Result<(), String> {
    let bytes: u64 = c
        .query_row(
            "SELECT COALESCE(sum(bytes),0) FROM personal_sources WHERE available=1",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if bytes > 64 * 1024 * 1024 {
        return Err("personal-tool-lineage-budget".into());
    }
    let mut after = 0;
    loop {
        let page = super::sources::page(c, after, 128)?;
        if page.is_empty() {
            break;
        }
        for seq in page {
            after = seq;
            let mut offset = 0;
            loop {
                let chunk = super::sources::load(c, seq, offset, 262144)?;
                offset = chunk.source.key.end;
                if !refs.iter().any(|s| s.key == chunk.source.key) {
                    refs.push(chunk.source);
                }
                if refs.len() > 4096 {
                    return Err("personal-tool-lineage-budget".into());
                }
                if offset == chunk.total_bytes {
                    break;
                }
            }
        }
    }
    Ok(())
}
/// New messages emitted by this run's UI tool may advance input_epoch. All other
/// new inputs invalidate the old request; edits/deletes are caught by SourceRef.
pub fn only_own_artifacts_since(c: &Connection, run: &str, sequence: u64) -> Result<bool, String> {
    c.query_row("SELECT NOT EXISTS(SELECT 1 FROM personal_sources s WHERE s.sequence>?2 AND NOT EXISTS(SELECT 1 FROM personal_artifacts a JOIN personal_generations g ON g.id=a.generation_id WHERE a.message_id=s.message_id AND g.run_id=?1))",rusqlite::params![run,sequence],|r|r.get(0)).map_err(database_error)
}
