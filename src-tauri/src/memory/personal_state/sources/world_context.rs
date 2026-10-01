//! Bounded same-Scope conversational evidence window.
use super::*;
/// Recent same-scope user evidence only. Context may inspect twelve metadata entries;
/// at most three additional finalized texts share the 32KiB generation budget.
pub fn world_context(
    c: &Connection,
    source: &SourceRef,
    scope: &str,
    primary_bytes: usize,
) -> Result<Vec<Chunk>, String> {
    let mut stmt = c.prepare("SELECT p.sequence FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version WHERE r.scope_key=?1 AND p.available=1 AND p.role IN ('user','transcript') AND p.sequence<?2 AND p.bytes BETWEEN 1 AND 32000 ORDER BY p.sequence DESC LIMIT 12").map_err(database_error)?;
    let sequences = stmt
        .query_map(params![scope, source.sequence], |r| r.get::<_, u64>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut budget = 32000usize.saturating_sub(primary_bytes);
    let mut result = Vec::new();
    for sequence in sequences {
        let chunk = load(c, sequence, 0, 32000)?;
        if !chunk.source.finalized || chunk.text.len() > budget {
            continue;
        }
        budget -= chunk.text.len();
        result.push(chunk);
        if result.len() == 3 {
            break;
        }
    }
    result.reverse();
    Ok(result)
}
