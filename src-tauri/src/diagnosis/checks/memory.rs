use super::{evidence, pass, CheckFuture};
use crate::diagnosis::contract::{Capability, Evidence, Outcome, Reason, Tier};
use crate::runtime::context::world::capabilities;
use crate::AppState;

const CAP: Capability = Capability::Memory;

pub(super) fn run(state: &AppState) -> CheckFuture<'_> {
    Box::pin(async move { collect(state) })
}

fn collect(state: &AppState) -> Vec<Evidence> {
    vec![
        personal_state(state),
        world(state),
        toolchain(state),
        context_still(
            "context-still.recall",
            state.context_still_recall.is_configured(),
        ),
        context_still(
            "context-still.search",
            state.context_still_search.is_configured(),
        ),
    ]
}

fn personal_state(state: &AppState) -> Evidence {
    let source = "memory.personal-state";
    match state
        .sqlite_readers
        .read(crate::memory::personal_state::commands::summary)
    {
        Ok(summary) if summary["enabled"] == false => evidence(
            source,
            CAP,
            Tier::Static,
            Outcome::Disabled,
            Reason::Disabled,
        ),
        Ok(summary) if summary["ready"] == true => pass(source, CAP, Tier::Static),
        Ok(summary) => evidence(
            source,
            CAP,
            Tier::Static,
            Outcome::Degraded,
            Reason::NotReady,
        )
        .detail(summary["contractReason"].as_str().unwrap_or("")),
        Err(error) => {
            evidence(source, CAP, Tier::Static, Outcome::Fail, Reason::Internal).detail(&error)
        }
    }
}

fn world(state: &AppState) -> Evidence {
    let source = "memory.world";
    let result = state.sqlite_readers.read(|connection| {
        capabilities::status(connection, crate::PRIMARY_CONVERSATION_ID)?;
        let summary = crate::memory::personal_state::commands::summary(connection)?;
        let active: u64 = connection
            .query_row(
                "SELECT count(*) FROM personal_projection p \
                 JOIN personal_assertions a ON a.id=p.assertion_id \
                 WHERE p.status='active' \
                 AND json_extract(a.metadata,'$.kind') IN ('world_entity','world_relation','world_focus')",
                [],
                |row| row.get(0),
            )
            .map_err(crate::database_error)?;
        Ok((summary, active))
    });
    match result {
        Ok((summary, _)) if summary["enabled"] != true => evidence(
            source,
            CAP,
            Tier::Static,
            Outcome::Disabled,
            Reason::Disabled,
        ),
        Ok((summary, active)) => {
            let ready = summary["ready"] == true;
            let maintenance = summary["maintenance"]["reason"]
                .as_str()
                .unwrap_or("not-started");
            let note = active.to_string();
            if !ready || maintenance.ends_with("unavailable") {
                evidence(
                    source,
                    CAP,
                    Tier::Static,
                    Outcome::Degraded,
                    Reason::NotReady,
                )
                .detail(&note)
            } else {
                pass(source, CAP, Tier::Static).detail(&note)
            }
        }
        Err(error) => {
            evidence(source, CAP, Tier::Static, Outcome::Fail, Reason::Internal).detail(&error)
        }
    }
}

fn toolchain(state: &AppState) -> Evidence {
    let source = "memory.toolchain";
    match state.sqlite_readers.read(|connection| {
        connection
            .query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(crate::database_error)
    }) {
        Ok(0) => evidence(
            source,
            CAP,
            Tier::Static,
            Outcome::Degraded,
            Reason::NotReady,
        )
        .advisory()
        .detail("0"),
        Ok(count) => pass(source, CAP, Tier::Static)
            .advisory()
            .detail(&count.to_string()),
        Err(error) => evidence(source, CAP, Tier::Static, Outcome::Fail, Reason::Internal)
            .advisory()
            .detail(&error),
    }
}

fn context_still(source: &str, configured: bool) -> Evidence {
    if configured {
        pass(source, CAP, Tier::Static).advisory()
    } else {
        evidence(
            source,
            CAP,
            Tier::Static,
            Outcome::Disabled,
            Reason::NotConfigured,
        )
        .advisory()
    }
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

    #[test]
    fn world_and_personal_state_distinguish_disabled_unready_and_ready() {
        let state = fresh();
        let summary = state
            .sqlite_writer
            .read_serialized(crate::memory::personal_state::commands::summary)
            .unwrap();
        let expected = if summary["enabled"] == false {
            Outcome::Disabled
        } else if summary["ready"] == false {
            Outcome::Degraded
        } else {
            Outcome::Pass
        };
        let items = collect(&state);
        for source in ["memory.personal-state", "memory.world"] {
            let item = items.iter().find(|item| item.source == source).unwrap();
            assert_eq!(item.outcome, expected, "{source}");
        }
    }

    #[test]
    fn every_memory_source_is_reported_once() {
        let items = collect(&fresh());
        let mut sources: Vec<_> = items.iter().map(|item| item.source.as_str()).collect();
        sources.sort_unstable();
        sources.dedup();
        assert_eq!(sources.len(), 5);
        assert!(items
            .iter()
            .all(|item| item.capability == Capability::Memory));
    }
}
