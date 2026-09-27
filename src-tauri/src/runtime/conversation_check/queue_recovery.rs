//! Recovery for speech that may already have reached the audio device.
use rusqlite::{params, Connection};

const UNCERTAIN_ERROR: &str = "生成途中の音声が中断されました。自動再試行はしません。";
const NO_ANSWER_ERROR: &str = "生成途中の音声が中断されました。再生できる確定回答がありません。";

pub(crate) fn mark_uncertain_speech(
    connection: &Connection,
    scope: &str,
    completed_at: &str,
    now_ms: i64,
) -> Result<(), String> {
    // A speech job with no saved answer may have emitted audio before the
    // process died. Its Qwen job must not retry, regardless of job state.
    connection
        .execute(
            "UPDATE task_queue_jobs AS q SET state='failed',owner=NULL,lease_until_ms=NULL,
             error=?3,updated_at_ms=?2
         WHERE q.scope=?1 AND q.kind='ornith_result' AND q.state IN ('queued','running')
           AND EXISTS (SELECT 1 FROM task_queue_jobs s
                       WHERE s.scope=q.scope AND s.job_key=q.job_key
                         AND s.generation=q.generation AND s.kind='speech' AND s.state IN ('interrupted','failed')
                         AND NOT EXISTS (SELECT 1 FROM conversation_messages m
                                         WHERE m.id='reply_' || q.job_key
                                           AND m.conversation_id=q.scope AND m.role='assistant'))",
            params![scope, now_ms, UNCERTAIN_ERROR],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE runtime_runs SET status='failed',error_message=?3,completed_at=?2
         WHERE status IN ('running','interrupted') AND id IN (
           SELECT 'run_' || s.job_key FROM task_queue_jobs s
           WHERE s.scope=?1 AND s.kind='speech' AND s.state IN ('interrupted','failed')
             AND NOT EXISTS (SELECT 1 FROM conversation_messages m
                             WHERE m.id='reply_' || s.job_key
                               AND m.conversation_id=s.scope AND m.role='assistant'))",
            params![scope, completed_at, UNCERTAIN_ERROR],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE task_queue_jobs SET state='failed',error=?3,updated_at_ms=?2
         WHERE scope=?1 AND kind='speech' AND state='interrupted'
           AND NOT EXISTS (SELECT 1 FROM conversation_messages m
                           WHERE m.id='reply_' || task_queue_jobs.job_key
                             AND m.conversation_id=task_queue_jobs.scope AND m.role='assistant')",
            params![scope, now_ms, NO_ANSWER_ERROR],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}
