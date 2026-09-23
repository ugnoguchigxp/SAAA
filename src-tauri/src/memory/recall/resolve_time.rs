use super::search::*;
use super::*;
use crate::memory::contracts::{
    RecallConversationInput, RecallConversationOutput, RecallError, RecallErrorCode,
    RecallTimeFilter, RecallTimePreset, MAX_RECALL_CALLS_PER_TURN, RECALL_NOTICE,
    RECALL_RETRIEVAL_MODE,
};
use chrono::{
    DateTime, Datelike, Duration, LocalResult, Months, NaiveDate, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};
pub(super) fn resolve_time(
    filter: &RecallTimeFilter,
    now: DateTime<Utc>,
    timezone: Tz,
) -> Result<ResolvedRange, RecallError> {
    match filter {
        RecallTimeFilter::Absolute { from, to_exclusive } => {
            let from = DateTime::parse_from_rfc3339(from)
                .map_err(|_| invalid_time())?
                .with_timezone(&Utc);
            let to = DateTime::parse_from_rfc3339(to_exclusive)
                .map_err(|_| invalid_time())?
                .with_timezone(&Utc);
            let from_ms = from.timestamp_millis();
            let to_exclusive_ms = to.timestamp_millis();
            if from >= to
                || from_ms >= to_exclusive_ms
                || to - from > Duration::days(MAX_ABSOLUTE_RANGE_DAYS)
            {
                return Err(invalid_time());
            }
            Ok(ResolvedRange {
                from_ms,
                to_exclusive_ms,
                timezone,
                label: "absolute".to_string(),
            })
        }
        RecallTimeFilter::Preset { preset } => {
            let local_now = now.with_timezone(&timezone);
            let today = local_now.date_naive();
            let (from, to) = match preset {
                RecallTimePreset::Today => (start_of_day(timezone, today)?, now),
                RecallTimePreset::Yesterday => {
                    let from_date = today - Duration::days(1);
                    (
                        start_of_day(timezone, from_date)?,
                        start_of_day(timezone, today)?,
                    )
                }
                RecallTimePreset::DayBeforeYesterday => {
                    let from_date = today - Duration::days(2);
                    let to_date = today - Duration::days(1);
                    (
                        start_of_day(timezone, from_date)?,
                        start_of_day(timezone, to_date)?,
                    )
                }
                RecallTimePreset::CurrentWeek => {
                    let monday = previous_or_same_monday(today);
                    (start_of_day(timezone, monday)?, now)
                }
                RecallTimePreset::PreviousCalendarWeek => {
                    let current_monday = previous_or_same_monday(today);
                    (
                        start_of_day(timezone, current_monday - Duration::days(7))?,
                        start_of_day(timezone, current_monday)?,
                    )
                }
                RecallTimePreset::Past7Days => (now - Duration::days(7), now),
                RecallTimePreset::PreviousCalendarMonth => {
                    let current_month = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                        .ok_or_else(invalid_time)?;
                    let previous_month = current_month
                        .checked_sub_months(Months::new(1))
                        .ok_or_else(invalid_time)?;
                    (
                        start_of_day(timezone, previous_month)?,
                        start_of_day(timezone, current_month)?,
                    )
                }
            };
            Ok(ResolvedRange {
                from_ms: from.timestamp_millis(),
                to_exclusive_ms: to.timestamp_millis(),
                timezone,
                label: preset.as_str().to_string(),
            })
        }
    }
}
pub(super) fn start_of_day(timezone: Tz, date: NaiveDate) -> Result<DateTime<Utc>, RecallError> {
    let midnight = date.and_hms_opt(0, 0, 0).ok_or_else(invalid_time)?;
    for minute in 0..1_440_i64 {
        let candidate = midnight + Duration::minutes(minute);
        match timezone.from_local_datetime(&candidate) {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    Err(invalid_time())
}
fn previous_or_same_monday(date: NaiveDate) -> NaiveDate {
    let days = match date.weekday() {
        Weekday::Mon => 0,
        weekday => i64::from(weekday.num_days_from_monday()),
    };
    date - Duration::days(days)
}
fn invalid_time() -> RecallError {
    RecallError::new(
        RecallErrorCode::InvalidTimeRange,
        "Time range is invalid or exceeds 366 days.",
    )
}
pub(super) fn filter_digest(query: Option<&str>, time: Option<&RecallTimeFilter>) -> String {
    let time_key = match time {
        None => "none".to_string(),
        Some(RecallTimeFilter::Preset { preset }) => format!("preset:{}", preset.as_str()),
        Some(RecallTimeFilter::Absolute { from, to_exclusive }) => {
            format!("absolute:{from}:{to_exclusive}")
        }
    };
    digest(&format!("{}\n{time_key}", query.unwrap_or_default()))
}
pub(super) fn load_cursor(
    connection: &Connection,
    runtime_run_id: &str,
    cursor: &str,
    expected_filter_digest: &str,
    now_ms: i64,
) -> Result<CursorState, RecallError> {
    let row = connection
        .query_row(
            "SELECT filter_digest,candidate_offset,range_from_ms,range_to_exclusive_ms,
                    timezone,range_label,expires_at_ms,snapshot_max_rowid
             FROM conversation_recall_cursors
             WHERE id=?1 AND runtime_run_id=?2",
            params![cursor, runtime_run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(local_unavailable)?
        .ok_or_else(|| {
            RecallError::new(
                RecallErrorCode::CursorFilterMismatch,
                "Cursor is missing or does not belong to this turn.",
            )
        })?;
    if row.0 != expected_filter_digest || row.6 <= now_ms {
        return Err(RecallError::new(
            RecallErrorCode::CursorFilterMismatch,
            "Cursor does not match the requested filters or has expired.",
        ));
    }
    let range = match (row.2, row.3, row.4, row.5) {
        (Some(from_ms), Some(to_exclusive_ms), Some(timezone), Some(label)) => {
            if from_ms >= to_exclusive_ms
                || DateTime::<Utc>::from_timestamp_millis(from_ms).is_none()
                || DateTime::<Utc>::from_timestamp_millis(to_exclusive_ms).is_none()
            {
                return Err(local_unavailable(""));
            }
            let timezone = timezone.parse::<Tz>().map_err(|_| local_unavailable(""))?;
            Some(ResolvedRange {
                from_ms,
                to_exclusive_ms,
                timezone,
                label,
            })
        }
        (None, None, None, None) => None,
        _ => return Err(local_unavailable("")),
    };
    if row.7 < 0 {
        return Err(local_unavailable(""));
    }
    Ok(CursorState {
        offset: usize::try_from(row.1).map_err(|_| local_unavailable(""))?,
        range,
        snapshot_max_rowid: row.7,
    })
}
pub(super) fn store_cursor(
    connection: &Connection,
    runtime_run_id: &str,
    filter_digest: &str,
    offset: usize,
    range: Option<&ResolvedRange>,
    snapshot_max_rowid: i64,
    now_ms: i64,
) -> Result<String, RecallError> {
    let id = format!("recall_cursor_{}", uuid::Uuid::new_v4().simple());
    connection
        .execute(
            "INSERT INTO conversation_recall_cursors(
               id,runtime_run_id,filter_digest,candidate_offset,range_from_ms,
               range_to_exclusive_ms,timezone,range_label,expires_at_ms,created_at_ms,
               snapshot_max_rowid
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                runtime_run_id,
                filter_digest,
                i64::try_from(offset).map_err(|_| local_unavailable(""))?,
                range.map(|value| value.from_ms),
                range.map(|value| value.to_exclusive_ms),
                range.map(|value| value.timezone.name()),
                range.map(|value| value.label.as_str()),
                now_ms.saturating_add(CURSOR_TTL_MS),
                now_ms,
                snapshot_max_rowid,
            ],
        )
        .map_err(local_unavailable)?;
    connection
        .execute(
            "DELETE FROM conversation_recall_cursors WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map_err(local_unavailable)?;
    Ok(id)
}
