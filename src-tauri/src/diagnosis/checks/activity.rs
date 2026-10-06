//! Evidence from recorded real use. Costs one cheap query per kind and sends nothing.
use super::{evidence, pass, CheckFuture};
use crate::diagnosis::contract::{now_ms, Capability, Evidence, Outcome, Reason, Tier};
use crate::AppState;
use rusqlite::OptionalExtension;

const RUN_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;
const AUDIT_WINDOW_MS: u64 = 60 * 60 * 1000;

pub(super) fn run(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move { collect(state, now_ms()) })
}

const RUN_KINDS: [(&str, Capability, &str); 4] = [
    (
        "conversation.respond",
        Capability::Conversation,
        "runs.conversation",
    ),
    (
        "voice.transcribe",
        Capability::VoiceListen,
        "runs.transcribe",
    ),
    ("voice.speak", Capability::VoiceSpeak, "runs.speak"),
    ("coding.assist", Capability::Coding, "runs.coding"),
];

fn collect(state: &AppState, now: u64) -> Vec<Evidence> {
    let mut out = Vec::new();
    let cutoff = now.saturating_sub(RUN_WINDOW_MS) as i64;
    for (kind, capability, source) in RUN_KINDS {
        let latest = state.sqlite_readers.read(|connection| {
            connection
                .query_row(
                    "SELECT status, COALESCE(failure_code, ''), \
                            CAST(COALESCE(completed_at, started_at) AS INTEGER) \
                     FROM runtime_runs \
                     WHERE route_kind=?1 AND status IN ('completed','failed') \
                       AND CAST(started_at AS INTEGER)>=?2 \
                     ORDER BY CAST(started_at AS INTEGER) DESC LIMIT 1",
                    rusqlite::params![kind, cutoff],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(crate::database_error)
        });
        if let Ok(Some((status, code, at))) = latest {
            out.push(run_evidence(
                source,
                capability,
                &status,
                &code,
                at.max(0) as u64,
            ));
        }
    }
    if let Some(item) = recent_asr(state, now) {
        out.push(item);
    }
    out
}

fn run_evidence(
    source: &str,
    capability: Capability,
    status: &str,
    code: &str,
    at: u64,
) -> Evidence {
    let mut item = if status == "completed" {
        pass(source, capability, Tier::Observed)
    } else {
        // One failed turn degrades; it must not read as an outage while other routes may work.
        evidence(
            source,
            capability,
            Tier::Observed,
            Outcome::Degraded,
            Reason::RecentFailure,
        )
        .detail(code)
    };
    item.observed_at = at;
    item
}

/// Capture failures are audited by the frontend as `conversation-asr-capture-start` with
/// phase `error`; transcription success as `asr-final-received`. Discards (no-speech, removed
/// TTS echo, cancels) are correct behaviour and never a fault. A later successful capture start
/// clears an earlier failure without counting as proof of transcription.
fn recent_asr(state: &AppState, now: u64) -> Option<Evidence> {
    let cutoff = now.saturating_sub(AUDIT_WINDOW_MS) as i64;
    let (event, phase, outcome, at) = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT event_name, phase, COALESCE(outcome, ''), CAST(occurred_at AS INTEGER) \
                     FROM audit_events \
                     WHERE component='voice-asr' AND CAST(occurred_at AS INTEGER)>=?1 \
                       AND event_name IN ('conversation-asr-capture-start','asr-final-received') \
                     ORDER BY sequence DESC LIMIT 1",
                    [cutoff],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(crate::database_error)
        })
        .ok()
        .flatten()?;
    let (source, capability) = ("asr.events", Capability::VoiceListen);
    let mut item = if event == "asr-final-received" {
        pass(source, capability, Tier::Observed)
    } else if phase == "error" || outcome == "failure" {
        evidence(
            source,
            capability,
            Tier::Observed,
            Outcome::Fail,
            Reason::CaptureFailed,
        )
    } else {
        return None;
    };
    item.observed_at = at.max(0) as u64;
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn insert_run(
        state: &AppState,
        id: &str,
        kind: &str,
        status: &str,
        code: Option<&str>,
        at: u64,
    ) {
        let (kind, status, id) = (kind.to_string(), status.to_string(), id.to_string());
        let code = code.map(str::to_string);
        state
            .sqlite_writer
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,failure_code,started_at,completed_at) \
                         VALUES(?1,?2,?3,?4,?5,?6,?6)",
                        rusqlite::params![id, crate::PRIMARY_CONVERSATION_ID, kind, status, code, at.to_string()],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("run inserts");
    }

    #[test]
    fn no_recorded_use_yields_no_evidence() {
        assert!(collect(&fresh(), now_ms()).is_empty());
    }

    #[test]
    fn latest_run_decides_and_failures_degrade() {
        let state = fresh();
        let now = now_ms();
        insert_run(
            &state,
            "r1",
            "conversation.respond",
            "completed",
            None,
            now - 3_000,
        );
        insert_run(
            &state,
            "r2",
            "conversation.respond",
            "failed",
            Some("provider-error"),
            now - 1_000,
        );
        insert_run(&state, "r3", "voice.speak", "completed", None, now - 2_000);
        let items = collect(&state, now);
        let conversation = items
            .iter()
            .find(|i| i.source == "runs.conversation")
            .unwrap();
        assert_eq!(conversation.outcome, Outcome::Degraded);
        assert_eq!(conversation.reason, Reason::RecentFailure);
        assert_eq!(conversation.observed_at, now - 1_000);
        let speak = items.iter().find(|i| i.source == "runs.speak").unwrap();
        assert_eq!(speak.outcome, Outcome::Pass);
    }

    fn insert_audit(state: &AppState, id: &str, event: &str, phase: &str, outcome: &str, at: u64) {
        let (id, event, phase, outcome) = (
            id.to_string(),
            event.to_string(),
            phase.to_string(),
            outcome.to_string(),
        );
        state
            .sqlite_writer
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json) \
                         VALUES(?1,?2,'voice-asr',?3,?4,?5,'{}')",
                        rusqlite::params![id, at.to_string(), event, phase, outcome],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("audit row inserts");
    }

    #[test]
    fn the_frontends_capture_failure_row_is_detected_and_a_later_start_clears_it() {
        let state = fresh();
        let now = now_ms();
        insert_audit(
            &state,
            "a1",
            "conversation-asr-capture-start",
            "error",
            "failure",
            now - 3_000,
        );
        let item = recent_asr(&state, now).expect("capture failure is observed");
        assert_eq!(
            (item.outcome, item.reason),
            (Outcome::Fail, Reason::CaptureFailed)
        );
        assert_eq!(item.observed_at, now - 3_000);
        insert_audit(
            &state,
            "a2",
            "conversation-asr-capture-start",
            "terminal",
            "success",
            now - 2_000,
        );
        assert!(recent_asr(&state, now).is_none());
        insert_audit(
            &state,
            "a3",
            "asr-final-received",
            "terminal",
            "success",
            now - 1_000,
        );
        assert_eq!(recent_asr(&state, now).unwrap().outcome, Outcome::Pass);
    }

    #[test]
    fn asr_discards_are_never_a_fault() {
        let state = fresh();
        let now = now_ms();
        insert_audit(
            &state,
            "d1",
            "asr-utterance-discarded",
            "decision",
            "blocked",
            now - 1_000,
        );
        assert!(recent_asr(&state, now).is_none());
    }

    #[test]
    fn runs_outside_the_window_are_ignored() {
        let state = fresh();
        let now = now_ms();
        insert_run(
            &state,
            "old",
            "conversation.respond",
            "completed",
            None,
            now - RUN_WINDOW_MS - 10,
        );
        assert!(collect(&state, now).is_empty());
    }
}
