//! Cron schedule support: parsing + next-occurrence computation,
//! shared by the scheduler loop and the API preview endpoint so the two never
//! drift. 5-field cron, or 6-field with a leading seconds field for sub-minute
//! schedules (floored at MIN_INTERVAL_SECS); evaluated in a per-schedule IANA
//! timezone (DST-aware via chrono-tz).

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use coveflow_types::RunKind;
use coveflow_types::schedule::Schedule;
use croner::Cron;
use sqlx::{PgPool, Postgres, Transaction};

use crate::QueueResult;
use crate::submit::{NewRun, submit_run_tx};

/// Hard cap on how many missed occurrences one tick will backfill for a single
/// `catchup` schedule, so a long downtime can't enqueue an unbounded burst.
const MAX_CATCHUP: usize = 50;

/// Smallest allowed interval between two fires (seconds). Sub-minute schedules
/// are supported (6-field cron with seconds) down to this floor; anything more
/// frequent is rejected — a DB-backed cron shouldn't fire faster than this.
pub const MIN_INTERVAL_SECS: i64 = 10;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScheduleError {
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),
    #[error("invalid timezone: {0}")]
    InvalidTimezone(String),
    #[error("schedule fires too frequently; minimum interval is {0}s")]
    TooFrequent(i64),
}

fn parse(cron_expr: &str, tz_name: &str) -> Result<(Cron, Tz), ScheduleError> {
    let tz: Tz = tz_name
        .parse()
        .map_err(|_| ScheduleError::InvalidTimezone(tz_name.to_string()))?;
    // `with_seconds_optional` accepts both standard 5-field and 6-field (leading
    // seconds) cron, so sub-minute schedules like `*/10 * * * * *` are allowed.
    let cron = Cron::new(cron_expr)
        .with_seconds_optional()
        .parse()
        .map_err(|e| ScheduleError::InvalidCron(e.to_string()))?;
    Ok((cron, tz))
}

/// Validate a cron expression + timezone, and enforce the [`MIN_INTERVAL_SECS`]
/// floor by sampling upcoming occurrences (rejects e.g. `*/5 * * * * *`).
pub fn validate(cron_expr: &str, tz_name: &str) -> Result<(), ScheduleError> {
    let (cron, tz) = parse(cron_expr, tz_name)?;
    // Sample enough fires to see the tightest gap (sub-minute crons repeat within
    // a minute, so ~12 covers it) and reject anything below the floor.
    let times = upcoming_with(&cron, &tz, Utc::now(), 12)?;
    for pair in times.windows(2) {
        if (pair[1] - pair[0]).num_milliseconds() < MIN_INTERVAL_SECS * 1000 {
            return Err(ScheduleError::TooFrequent(MIN_INTERVAL_SECS));
        }
    }
    Ok(())
}

/// The next `n` trigger instants strictly after `after`, evaluated in `tz_name`,
/// returned in UTC (ascending). Powers the editor's "next runs" preview.
pub fn upcoming(
    cron_expr: &str,
    tz_name: &str,
    after: DateTime<Utc>,
    n: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleError> {
    let (cron, tz) = parse(cron_expr, tz_name)?;
    upcoming_with(&cron, &tz, after, n)
}

/// `upcoming` over an already-parsed cron + tz (so callers that parse once —
/// like `validate` — don't re-parse).
fn upcoming_with(
    cron: &Cron,
    tz: &Tz,
    after: DateTime<Utc>,
    n: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleError> {
    let mut out = Vec::with_capacity(n);
    let mut cursor = after.with_timezone(tz);
    for _ in 0..n {
        let next = cron
            .find_next_occurrence(&cursor, false)
            .map_err(|e| ScheduleError::InvalidCron(e.to_string()))?;
        out.push(next.with_timezone(&Utc));
        cursor = next;
    }
    Ok(out)
}

/// The single next trigger instant strictly after `after` (UTC), or `None` if
/// the cron never fires again (e.g. a one-shot date already past).
pub fn next_after(
    cron_expr: &str,
    tz_name: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ScheduleError> {
    Ok(upcoming(cron_expr, tz_name, after, 1)?.into_iter().next())
}

/// `(occurrences_to_fire, next_trigger_at, truncated)` — the plan for one tick.
type FirePlan = (Vec<DateTime<Utc>>, Option<DateTime<Utc>>, bool);

/// Decide which occurrences a due schedule should fire this tick and the new
/// `next_trigger_at`. `due` is the schedule's current `next_trigger_at` (already
/// `<= now`).
fn plan_fires(
    cron_expr: &str,
    tz: &str,
    due: DateTime<Utc>,
    now: DateTime<Utc>,
    catchup: bool,
) -> Result<FirePlan, ScheduleError> {
    if !catchup {
        // Fire the due occurrence once; skip any others missed during downtime.
        return Ok((vec![due], next_after(cron_expr, tz, now)?, false));
    }
    let mut occ = vec![due];
    let mut cursor = due;
    loop {
        let Some(next) = next_after(cron_expr, tz, cursor)? else {
            return Ok((occ, None, false));
        };
        if next > now {
            return Ok((occ, Some(next), false));
        }
        if occ.len() >= MAX_CATCHUP {
            // Too many missed; backfill the cap then skip ahead past now.
            return Ok((occ, next_after(cron_expr, tz, now)?, true));
        }
        occ.push(next);
        cursor = next;
    }
}

/// One scheduler pass: claim every due schedule (`FOR UPDATE SKIP LOCKED`, so
/// each row is processed by exactly one instance per tick), fire its flow run(s)
/// and advance `next_trigger_at` — all in one transaction. Returns the number of
/// runs fired. Run by `spawn_scheduler` in the binary on a fixed interval.
#[tracing::instrument(name = "queue::run_due_schedules", skip(db))]
pub async fn run_due_schedules(db: &PgPool, now: DateTime<Utc>) -> QueueResult<usize> {
    let mut tx = db.begin().await?;
    let due = sqlx::query_as!(
        Schedule,
        r#"SELECT id, workspace_id, name, flow_id, cron_expr, timezone,
                  args, enabled, catchup, max_active_runs, next_trigger_at,
                  last_triggered_at, last_error, created_by, created_at, updated_at
           FROM schedule
           WHERE enabled = TRUE AND next_trigger_at IS NOT NULL AND next_trigger_at <= $1
           ORDER BY next_trigger_at
           LIMIT 100
           FOR UPDATE SKIP LOCKED"#,
        now
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut fired_total = 0usize;
    for s in &due {
        fired_total += process_one(&mut tx, s, now).await?;
    }
    tx.commit().await?;
    Ok(fired_total)
}

#[tracing::instrument(name = "queue::schedule_fire", skip(tx, s), fields(schedule_id = %s.id, flow_id = %s.flow_id))]
async fn process_one(
    tx: &mut Transaction<'_, Postgres>,
    s: &Schedule,
    now: DateTime<Utc>,
) -> QueueResult<usize> {
    // Snapshot the flow's latest revision (by stable id) at fire time, resolving
    // its current path for the run record.
    let flow = sqlx::query!(
        "SELECT value, path FROM flow WHERE workspace_id = $1 AND flow_id = $2
         ORDER BY revision DESC LIMIT 1",
        s.workspace_id,
        s.flow_id
    )
    .fetch_optional(&mut **tx)
    .await?;

    let Some(flow) = flow else {
        // Flow gone (deleted without removing the schedule):
        // advance so we don't spin, and surface why nothing ran.
        let next = next_after(&s.cron_expr, &s.timezone, now).ok().flatten();
        sqlx::query!(
            "UPDATE schedule SET next_trigger_at = $1, last_error = $2, updated_at = now() WHERE id = $3",
            next,
            format!("flow '{}' not found", s.flow_id),
            s.id
        )
        .execute(&mut **tx)
        .await?;
        return Ok(0);
    };
    let value = flow.value;
    let flow_path = flow.path;

    let due = s.next_trigger_at.unwrap_or(now);
    let (occurrences, next_trigger, truncated) =
        match plan_fires(&s.cron_expr, &s.timezone, due, now, s.catchup) {
            Ok(plan) => plan,
            Err(e) => {
                sqlx::query!(
                    "UPDATE schedule SET last_error = $1, updated_at = now() WHERE id = $2",
                    e.to_string(),
                    s.id
                )
                .execute(&mut **tx)
                .await?;
                return Ok(0);
            }
        };

    // Active = runs of this schedule not yet terminal (queued or running).
    let mut active = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM run r
           WHERE r.schedule_id = $1
             AND NOT EXISTS (SELECT 1 FROM run_completed c WHERE c.id = r.id)"#,
        s.id
    )
    .fetch_one(&mut **tx)
    .await?;

    let mut fired: i64 = 0;
    for occ in &occurrences {
        if let Some(max) = s.max_active_runs {
            if active >= max as i64 {
                break; // at capacity → skip remaining fires this tick
            }
        }
        submit_run_tx(
            tx,
            NewRun {
                workspace_id: &s.workspace_id,
                kind: RunKind::Flow,
                script_hash: None,
                script_path: Some(&flow_path),
                raw_code: None,
                language: None,
                args: Some(s.args.clone()),
                flow_value: Some(value.clone()),
                tag: "default",
                parent_run: None,
                root_run: None,
                flow_step_id: None,
                team_owner: None,
                created_by: &s.created_by,
                trace_id: None,
                span_id: None,
                scheduled_for: None,
                priority: None,
                cpus: None,
                memory_mb: None,
                disk_mb: None,
                requirements: vec![],
                timeout: None,
                custom_image: None,
                schedule_id: Some(s.id),
                // Airflow-style logical date: the cron slot this run is for.
                scheduled_time: Some(*occ),
                // Data interval end = the next cron slot after this one, snapshotted
                // now so it survives later edits/deletes of the schedule.
                data_interval_end: next_after(&s.cron_expr, &s.timezone, *occ).ok().flatten(),
                trigger_id: None,
                trigger_context: None,
            },
        )
        .await?;
        active += 1;
        fired += 1;
    }

    // If max_active_runs throttled us mid-catchup we fired fewer than we planned.
    // Park next_trigger_at on the first un-fired slot so the remaining catchup
    // occurrences are retried next tick once a run frees capacity — otherwise they
    // are silently dropped (next_trigger would jump past now). Only meaningful for
    // catchup; non-catchup intentionally skips missed slots.
    let fired_n = fired as usize;
    let throttled = s.catchup && fired_n < occurrences.len();
    let next_trigger = if throttled {
        Some(occurrences[fired_n])
    } else {
        next_trigger
    };

    let last_error: Option<String> = if throttled {
        Some("throttled: max_active_runs reached, deferring catchup".to_string())
    } else if truncated {
        Some(format!("catchup truncated at {MAX_CATCHUP}"))
    } else if fired == 0 && !occurrences.is_empty() {
        Some("skipped: max_active_runs reached".to_string())
    } else {
        None
    };
    sqlx::query!(
        "UPDATE schedule
         SET next_trigger_at = $1,
             last_triggered_at = CASE WHEN $2 THEN $3 ELSE last_triggered_at END,
             last_error = $4,
             updated_at = now()
         WHERE id = $5",
        next_trigger,
        fired > 0,
        now,
        last_error,
        s.id
    )
    .execute(&mut **tx)
    .await?;

    Ok(fired as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn next_after_daily_utc() {
        // Daily at 02:00 UTC; from 00:00 → same day 02:00.
        let after = Utc.with_ymd_and_hms(2026, 6, 8, 0, 0, 0).unwrap();
        let next = next_after("0 2 * * *", "UTC", after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 8, 2, 0, 0).unwrap());
    }

    #[test]
    fn upcoming_is_ascending_and_strict() {
        let after = Utc.with_ymd_and_hms(2026, 6, 8, 2, 0, 0).unwrap();
        let next = upcoming("0 2 * * *", "UTC", after, 3).unwrap();
        assert_eq!(next.len(), 3);
        // strictly after `after` (02:00 already passed → first is next day)
        assert_eq!(next[0], Utc.with_ymd_and_hms(2026, 6, 9, 2, 0, 0).unwrap());
        assert!(next[0] < next[1] && next[1] < next[2]);
    }

    #[test]
    fn timezone_is_honored() {
        // Midnight in Asia/Taipei (UTC+8) == 16:00 UTC the previous day.
        let after = Utc.with_ymd_and_hms(2026, 6, 8, 0, 0, 0).unwrap();
        let next = next_after("0 0 * * *", "Asia/Taipei", after)
            .unwrap()
            .unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 6, 8, 16, 0, 0).unwrap());
    }

    #[test]
    fn dst_spring_forward_shifts_utc_offset() {
        use chrono::Timelike;
        // America/New_York springs forward 2026-03-08 (EST UTC-5 → EDT UTC-4).
        // Daily noon local: noon EST = 17:00 UTC, noon EDT = 16:00 UTC. A
        // fixed-UTC-offset (DST-unaware) impl would keep the same UTC hour.
        let after = Utc.with_ymd_and_hms(2026, 3, 6, 0, 0, 0).unwrap();
        let next = upcoming("0 12 * * *", "America/New_York", after, 5).unwrap();
        assert_eq!(next[0].hour(), 17, "Mar 6 noon EST = 17:00 UTC");
        assert_eq!(
            next[2].hour(),
            16,
            "Mar 8 noon EDT = 16:00 UTC (clocks moved)"
        );
    }

    #[test]
    fn dst_fall_back_shifts_utc_offset() {
        use chrono::Timelike;
        // America/New_York falls back 2026-11-01 (EDT UTC-4 → EST UTC-5).
        let after = Utc.with_ymd_and_hms(2026, 10, 30, 0, 0, 0).unwrap();
        let next = upcoming("0 12 * * *", "America/New_York", after, 5).unwrap();
        assert_eq!(next[0].hour(), 16, "Oct 30 noon EDT = 16:00 UTC");
        assert_eq!(
            next[2].hour(),
            17,
            "Nov 1 noon EST = 17:00 UTC (clocks moved)"
        );
    }

    #[test]
    fn bad_cron_rejected() {
        let err = validate("not a cron", "UTC").unwrap_err();
        assert!(matches!(err, ScheduleError::InvalidCron(_)));
    }

    #[test]
    fn bad_timezone_rejected() {
        let err = validate("0 2 * * *", "Mars/Phobos").unwrap_err();
        assert_eq!(err, ScheduleError::InvalidTimezone("Mars/Phobos".into()));
    }

    #[test]
    fn sub_minute_every_10s_allowed() {
        // 6-field cron (leading seconds): every 10s is the floor → accepted.
        validate("*/10 * * * * *", "UTC").unwrap();
        let after = Utc.with_ymd_and_hms(2026, 6, 8, 0, 0, 0).unwrap();
        let next = upcoming("*/10 * * * * *", "UTC", after, 3).unwrap();
        assert_eq!(next[0], Utc.with_ymd_and_hms(2026, 6, 8, 0, 0, 10).unwrap());
        assert_eq!(next[1], Utc.with_ymd_and_hms(2026, 6, 8, 0, 0, 20).unwrap());
    }

    #[test]
    fn too_frequent_rejected() {
        // Faster than the 10s floor → rejected.
        assert_eq!(
            validate("*/5 * * * * *", "UTC").unwrap_err(),
            ScheduleError::TooFrequent(10)
        );
        assert_eq!(
            validate("* * * * * *", "UTC").unwrap_err(),
            ScheduleError::TooFrequent(10)
        );
        // Standard minute cron still fine.
        validate("* * * * *", "UTC").unwrap();
    }
}
