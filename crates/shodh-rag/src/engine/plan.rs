//! What a job's schedule asks for now. Pure: the engine reads the job, its active run and
//! the latest occurrence already handled, and applies the step this returns.
//!
//! - **Catch-up.** An occurrence more than [`LATE_AFTER_MINUTES`] late means the computer
//!   slept or Shodh was closed. With `once`, one run for the latest missed time, counting
//!   the earlier ones as `missed`; with `skip`, a skipped run is recorded.
//! - **Overlap.** A job has one active run. When it waits for the user's decision, or
//!   waits through no failure (while the user chats, until a quota resets), the new
//!   occurrences fold into it (one run, never a pile). Otherwise `skip` records
//!   that the previous run was still going, and `queue_one` leaves the occurrence for
//!   when the active run ends (so at most one runs behind it).

use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::schedule::Schedule;
use super::store::{CatchUp, Overlap, Run, RunState, Trigger, WaitReason};

/// Longest the engine sleeps before looking at the clock again. Sleeping is measured on
/// a monotonic clock, which a suspended computer or a clock change does not move like the
/// wall clock; looking every minute bounds how late a run can be after either.
pub const MAX_SLEEP: Duration = Duration::from_secs(60);
/// How late an occurrence may be and still count as on time.
pub const LATE_AFTER_MINUTES: i64 = 5;
/// The most occurrences counted at once (every hour over a long sleep).
pub const OCCURRENCE_CAP: usize = 1000;
/// Failed runs in a row before a job is put on hold.
pub const FAILED_RUNS_BEFORE_HOLD: u32 = 3;

/// What to do for a job now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Queue a run for `scheduled_for`, standing for `missed` earlier occurrences too.
    Start {
        trigger: Trigger,
        scheduled_for: DateTime<Utc>,
        missed: u32,
    },
    /// Record a skipped run for `scheduled_for`.
    RecordSkipped {
        trigger: Trigger,
        scheduled_for: DateTime<Utc>,
        missed: u32,
        summary: String,
    },
    /// Move the waiting decision `run_id` to `to`, adding `add_missed` occurrences.
    Fold {
        run_id: String,
        to: DateTime<Utc>,
        add_missed: u32,
    },
}

/// Whether a run waiting for `reason` takes newer occurrences in (one run, never a pile):
/// the user's decision, and the waits that are no failure (while the user chats, until a
/// quota resets, until the automations' model answers). A run waiting for an approval, a
/// backoff or the network keeps its occurrence, so the overlap policy applies.
fn folds(reason: WaitReason) -> bool {
    matches!(
        reason,
        WaitReason::Decision | WaitReason::Yield | WaitReason::Quota | WaitReason::ModelUnavailable
    )
}

/// The step for a job with `schedule`, whose active run is `active` and whose latest
/// handled occurrence is `planned_through`, at `now`, in `tz`.
pub fn plan<Tz>(
    tz: &Tz,
    schedule: &Schedule,
    overlap: Overlap,
    catch_up: CatchUp,
    active: Option<&Run>,
    planned_through: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Step
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    let (due, more) = schedule.occurrences_in(tz, planned_through, now, OCCURRENCE_CAP);
    let Some(&latest) = due.last() else {
        return Step::Nothing;
    };
    let count = u32::try_from(due.len())
        .unwrap_or(u32::MAX)
        .saturating_add(u32::from(more));
    let earlier = count.saturating_sub(1);
    if let Some(run) = active {
        if run.state == RunState::Waiting && run.wait_reason.is_some_and(folds) {
            return Step::Fold {
                run_id: run.id.clone(),
                to: latest,
                add_missed: count,
            };
        }
        return match overlap {
            Overlap::Skip => Step::RecordSkipped {
                trigger: Trigger::Schedule,
                scheduled_for: latest,
                missed: earlier,
                summary: "Skipped: the previous run was still going.".to_string(),
            },
            Overlap::QueueOne => Step::Nothing,
        };
    }
    let late = now - latest > TimeDelta::minutes(LATE_AFTER_MINUTES);
    if late && catch_up == CatchUp::Skip {
        return Step::RecordSkipped {
            trigger: Trigger::CatchUp,
            scheduled_for: latest,
            missed: earlier,
            summary: format!(
                "Skipped: Shodh wasn't running at {}.",
                latest.with_timezone(tz).format("%a %-d %b, %H:%M")
            ),
        };
    }
    Step::Start {
        trigger: if late {
            Trigger::CatchUp
        } else {
            Trigger::Schedule
        },
        scheduled_for: latest,
        missed: earlier,
    }
}

/// How long to sleep before the next plan: until `next`, at most [`MAX_SLEEP`].
pub fn sleep_for(next: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Duration {
    match next {
        Some(at) => (at - now).to_std().unwrap_or(Duration::ZERO).min(MAX_SLEEP),
        None => MAX_SLEEP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn utc(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, d, h, m, 0).unwrap()
    }

    fn daily() -> Schedule {
        Schedule::Daily {
            at: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        }
    }

    #[test]
    fn nothing_is_due_before_the_next_occurrence() {
        let step = plan(
            &Utc,
            &daily(),
            Overlap::Skip,
            CatchUp::Once,
            None,
            utc(20, 9, 0),
            utc(21, 8, 59),
        );
        assert_eq!(step, Step::Nothing);
    }

    #[test]
    fn an_on_time_occurrence_starts_a_scheduled_run() {
        let step = plan(
            &Utc,
            &daily(),
            Overlap::Skip,
            CatchUp::Once,
            None,
            utc(20, 9, 0),
            utc(21, 9, 4),
        );
        assert_eq!(
            step,
            Step::Start {
                trigger: Trigger::Schedule,
                scheduled_for: utc(21, 9, 0),
                missed: 0
            }
        );
    }

    #[test]
    fn a_late_occurrence_is_a_catch_up() {
        let step = plan(
            &Utc,
            &daily(),
            Overlap::Skip,
            CatchUp::Once,
            None,
            utc(20, 9, 0),
            utc(23, 12, 0),
        );
        assert_eq!(
            step,
            Step::Start {
                trigger: Trigger::CatchUp,
                scheduled_for: utc(23, 9, 0),
                missed: 2
            }
        );
    }

    #[test]
    fn sleep_is_capped_at_a_minute() {
        let now = utc(21, 9, 0);
        assert_eq!(sleep_for(None, now), MAX_SLEEP);
        assert_eq!(sleep_for(Some(utc(21, 10, 0)), now), MAX_SLEEP);
        assert_eq!(
            sleep_for(Some(now + TimeDelta::seconds(30)), now),
            Duration::from_secs(30)
        );
        assert_eq!(sleep_for(Some(utc(21, 8, 0)), now), Duration::ZERO);
    }
}
