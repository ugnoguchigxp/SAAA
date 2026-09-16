use crate::{database_error, runtime::pi::runner::identity};
use rusqlite::{params, Connection};
use serde_json::json;
// Never resend an ambiguous delivery, and never signal an unverified PID.
pub fn reconcile(c: &Connection) -> Result<(), String> {
    let mut stmt=c.prepare("SELECT id,job_id,pid,process_identity,delivery FROM coding_runs WHERE state IN ('starting','running','stopping','outcome_unknown')").map_err(database_error)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<u32>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (run, job, pid, saved, delivery) in rows {
        let live = pid.map(identity).unwrap_or_default();
        // Unknown ownership remains blocked while any process occupies the PID.
        let unresolved = pid.is_some_and(process_may_exist)
            || (pid.is_none() && (delivery != "prepared" || saved.as_deref() == Some("launching")));
        let state = if unresolved {
            "outcome_unknown"
        } else {
            "interrupted"
        };
        c.execute("UPDATE coding_runs SET state=?2,delivery=CASE WHEN delivery IN ('sending','accepted') THEN 'unknown' ELSE delivery END,result_json=?3 WHERE id=?1",params![run,state,json!({"error":if unresolved{"old_process_unresolved"}else{"app_restarted"},"ownershipVerified":saved.as_ref().is_some_and(|s|!s.is_empty() && *s==live),"complete":false}).to_string()]).map_err(database_error)?;
        c.execute(
            "UPDATE coding_jobs SET state=?2,revision=revision+1 WHERE id=?1",
            params![job, state],
        )
        .map_err(database_error)?;
        super::repository::event(c, &job, &run, state, json!({"reason":"app_restarted"}))?;
    }
    Ok(())
}

fn process_may_exist(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 checks existence without signalling or trusting ps output.
        if unsafe { libc::kill(pid as i32, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}
