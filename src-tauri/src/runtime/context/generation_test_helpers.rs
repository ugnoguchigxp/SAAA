#![cfg(test)]
use super::*;
pub(crate) fn assert_two_round_tool_manifest(state: &crate::AppState, run_id: &str) {
    state
        .sqlite_readers
        .read(|connection| {
            let manifest: (u64, u64, u64) = connection
                .query_row(
                    "SELECT COUNT(*),
                       SUM(purpose='reasoning' AND status='completed'),
                       SUM(purpose='tool-followup' AND status='completed')
                     FROM context_generations WHERE run_id=?1",
                    [run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(database_error)?;
            assert_eq!(manifest, (2, 1, 1));
            let invalid: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM context_generations
                     WHERE run_id=?1 AND current_instruction_count!=1)",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            assert!(!invalid);
            let tool_snapshots: u64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM context_generation_inputs i
                     JOIN context_generations g ON g.id=i.generation_id
                     WHERE g.run_id=?1 AND i.source_kind='static-tool-offer'
                       AND i.selected=1 AND length(i.source_digest)=64",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            assert!(tool_snapshots > 0);
            Ok(())
        })
        .unwrap();
}
