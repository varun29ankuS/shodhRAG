//! Typed schedules: when a job is due. The interface offers plain choices (every day,
//! weekdays, chosen days of the week, a day of the month or its last day, every few
//! hours, once); there is no expression to type and none to show.
//!
//! Calendar schedules are local wall-clock times: each date's time is turned into an
//! instant with [`resolve_local`] on every computation, never cached, so a time-zone or
//! daylight-saving change applies at once. A time that does not exist (clocks went
//! forward) is the first minute after the gap; a time that happens twice (clocks went
//! back) is its first instance, so a day never has two occurrences. Every few hours
//! counts real hours from a fixed instant, so daylight saving never shifts it.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::clock::resolve_local;

/// The most dates walked to find the next occurrence of a calendar schedule. The
/// longest real gap is a month (a monthly day), so this only bounds a broken schedule.
const MAX_DAYS_AHEAD: i64 = 800;

/// A schedule that cannot be used, in words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScheduleError(pub String);

/// A day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekDay {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl WeekDay {
    pub const ALL: [WeekDay; 7] = [
        Self::Mon,
        Self::Tue,
        Self::Wed,
        Self::Thu,
        Self::Fri,
        Self::Sat,
        Self::Sun,
    ];

    pub fn of(date: NaiveDate) -> Self {
        Self::ALL[date.weekday().num_days_from_monday() as usize]
    }

    /// The short English name: `Mon`.
    pub fn short(self) -> &'static str {
        match self {
            Self::Mon => "Mon",
            Self::Tue => "Tue",
            Self::Wed => "Wed",
            Self::Thu => "Thu",
            Self::Fri => "Fri",
            Self::Sat => "Sat",
            Self::Sun => "Sun",
        }
    }

    /// The full English name: `Monday`.
    pub fn long(self) -> &'static str {
        match self {
            Self::Mon => "Monday",
            Self::Tue => "Tuesday",
            Self::Wed => "Wednesday",
            Self::Thu => "Thursday",
            Self::Fri => "Friday",
            Self::Sat => "Saturday",
            Self::Sun => "Sunday",
        }
    }

    fn is_weekday(self) -> bool {
        !matches!(self, Self::Sat | Self::Sun)
    }
}

/// The day of a monthly schedule: a day number (a month without it uses its last day),
/// or the last day. Stored as the number, or `"last"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonthDay {
    Day(u32),
    Last,
}

impl Serialize for MonthDay {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Day(n) => serializer.serialize_u32(*n),
            Self::Last => serializer.serialize_str("last"),
        }
    }
}

impl<'de> Deserialize<'de> for MonthDay {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u32),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Number(n) => Ok(Self::Day(n)),
            Raw::Text(text) if text == "last" => Ok(Self::Last),
            Raw::Text(text) => Err(serde::de::Error::custom(format!(
                "a monthly day is a number from 1 to 31 or \"last\", not {text:?}"
            ))),
        }
    }
}

/// `HH:MM` for times of day.
mod hhmm {
    use chrono::NaiveTime;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &NaiveTime, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&at.format("%H:%M").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<NaiveTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        NaiveTime::parse_from_str(&text, "%H:%M")
            .map_err(|_| serde::de::Error::custom(format!("a time is HH:MM, not {text:?}")))
    }
}

/// `YYYY-MM-DDTHH:MM` for a local date and time.
mod local_minute {
    use chrono::NaiveDateTime;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&at.format("%Y-%m-%dT%H:%M").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<NaiveDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M").map_err(|_| {
            serde::de::Error::custom(format!("a local time is YYYY-MM-DDTHH:MM, not {text:?}"))
        })
    }
}

/// When a job is due. Stored as tagged JSON (`{"kind": "weekly", "days": ["mon"],
/// "at": "09:00"}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Once, at a local date and time.
    Once {
        #[serde(with = "local_minute")]
        at: NaiveDateTime,
    },
    /// Every day at a local time.
    Daily {
        #[serde(with = "hhmm")]
        at: NaiveTime,
    },
    /// Monday to Friday at a local time.
    Weekdays {
        #[serde(with = "hhmm")]
        at: NaiveTime,
    },
    /// On the chosen days of every week at a local time.
    Weekly {
        days: Vec<WeekDay>,
        #[serde(with = "hhmm")]
        at: NaiveTime,
    },
    /// On a day of every month at a local time.
    Monthly {
        day: MonthDay,
        #[serde(with = "hhmm")]
        at: NaiveTime,
    },
    /// Every `hours` real hours from `from`.
    EveryHours { hours: u32, from: DateTime<Utc> },
}

/// The number of days in the month of `date`.
fn days_in_month(date: NaiveDate) -> u32 {
    let (year, month) = (date.year(), date.month());
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.and_then(|n| n.pred_opt())
        .map_or(28, |last| last.day())
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The key of an occurrence in local time, `yyyymmddhhmm`: the suffix of the ids of what
/// a job makes for it, so one occurrence always gives the same id.
pub fn local_key<Tz: TimeZone>(tz: &Tz, at: DateTime<Utc>) -> String {
    at.with_timezone(tz)
        .naive_local()
        .format("%Y%m%d%H%M")
        .to_string()
}

impl Schedule {
    /// Days sorted and without repeats; a schedule as the engine stores it.
    pub fn normalized(self) -> Self {
        match self {
            Self::Weekly { mut days, at } => {
                days.sort();
                days.dedup();
                Self::Weekly { days, at }
            }
            other => other,
        }
    }

    /// Whether the schedule can be used.
    pub fn validate(&self) -> Result<(), ScheduleError> {
        match self {
            Self::Weekly { days, .. } if days.is_empty() => Err(ScheduleError(
                "A weekly repeat needs at least one day.".to_string(),
            )),
            Self::Monthly {
                day: MonthDay::Day(n),
                ..
            } if !(1..=31).contains(n) => Err(ScheduleError(format!(
                "A monthly day is from 1 to 31, not {n}."
            ))),
            Self::EveryHours { hours, .. } if !(1..=24).contains(hours) => Err(ScheduleError(
                format!("Every few hours means 1 to 24 hours, not {hours}."),
            )),
            _ => Ok(()),
        }
    }

    /// The local time of day of a calendar schedule.
    pub fn time_of_day(&self) -> Option<NaiveTime> {
        match self {
            Self::Once { at } => Some(at.time()),
            Self::Daily { at }
            | Self::Weekdays { at }
            | Self::Weekly { at, .. }
            | Self::Monthly { at, .. } => Some(*at),
            Self::EveryHours { .. } => None,
        }
    }

    /// The same days at another time of day (every few hours is left as it is).
    pub fn at_time(self, time: NaiveTime) -> Self {
        match self {
            Self::Once { at } => Self::Once {
                at: at.date().and_time(time),
            },
            Self::Daily { .. } => Self::Daily { at: time },
            Self::Weekdays { .. } => Self::Weekdays { at: time },
            Self::Weekly { days, .. } => Self::Weekly { days, at: time },
            Self::Monthly { day, .. } => Self::Monthly { day, at: time },
            every @ Self::EveryHours { .. } => every,
        }
    }

    /// Whether a calendar schedule falls on `date`.
    fn falls_on(&self, date: NaiveDate) -> bool {
        match self {
            Self::Once { at } => at.date() == date,
            Self::Daily { .. } => true,
            Self::Weekdays { .. } => WeekDay::of(date).is_weekday(),
            Self::Weekly { days, .. } => days.contains(&WeekDay::of(date)),
            Self::Monthly { day, .. } => {
                let last = days_in_month(date);
                let wanted = match day {
                    MonthDay::Day(n) => (*n).min(last),
                    MonthDay::Last => last,
                };
                date.day() == wanted
            }
            Self::EveryHours { .. } => false,
        }
    }

    /// The first occurrence strictly after `after`, in `tz`; `None` when there is none (a
    /// past once, or an invalid schedule).
    pub fn next_after<Tz: TimeZone>(&self, tz: &Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if self.validate().is_err() {
            return None;
        }
        if let Self::EveryHours { hours, from } = self {
            let step = Duration::hours(i64::from(*hours));
            if after < *from {
                return Some(*from);
            }
            let steps = (after - *from).num_seconds() / step.num_seconds() + 1;
            let steps = i32::try_from(steps).ok()?;
            return Some(*from + step * steps);
        }
        if let Self::Once { at } = self {
            return resolve_local(tz, *at).filter(|at| *at > after);
        }
        let time = self.time_of_day()?;
        // A day before `after`'s local date: a local time early on that date can map to an
        // instant just after `after` across a zone change.
        let mut date = after.with_timezone(tz).date_naive().pred_opt()?;
        for _ in 0..MAX_DAYS_AHEAD {
            if self.falls_on(date) {
                if let Some(at) = resolve_local(tz, date.and_time(time)) {
                    if at > after {
                        return Some(at);
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// The occurrences in `(after, upto]`, oldest first, at most `cap` of them, and whether
    /// more fell in the window.
    pub fn occurrences_in<Tz: TimeZone>(
        &self,
        tz: &Tz,
        after: DateTime<Utc>,
        upto: DateTime<Utc>,
        cap: usize,
    ) -> (Vec<DateTime<Utc>>, bool) {
        let mut out = Vec::new();
        let mut from = after;
        while let Some(next) = self.next_after(tz, from) {
            if next > upto {
                break;
            }
            if out.len() == cap {
                return (out, true);
            }
            out.push(next);
            from = next;
        }
        (out, false)
    }

    /// In words, with or without the time of day: "Every week on Mon, Thu at 09:00".
    pub fn label(&self, with_time: bool) -> String {
        let time = |at: &NaiveTime| {
            if with_time {
                format!(" at {}", at.format("%H:%M"))
            } else {
                String::new()
            }
        };
        match self {
            Self::Once { at } => {
                if with_time {
                    format!("Once, on {}", at.format("%a %-d %b %Y at %H:%M"))
                } else {
                    format!("Once, on {}", at.format("%a %-d %b %Y"))
                }
            }
            Self::Daily { at } => format!("Every day{}", time(at)),
            Self::Weekdays { at } => format!("Every weekday{}", time(at)),
            Self::Weekly { days, at } => {
                let names: Vec<&str> = days.iter().map(|d| d.short()).collect();
                format!("Every week on {}{}", names.join(", "), time(at))
            }
            Self::Monthly {
                day: MonthDay::Day(n),
                at,
            } => format!("Every month on the {}{}", ordinal(*n), time(at)),
            Self::Monthly {
                day: MonthDay::Last,
                at,
            } => format!("Every month on the last day{}", time(at)),
            Self::EveryHours { hours: 1, .. } => "Every hour".to_string(),
            Self::EveryHours { hours, .. } => format!("Every {hours} hours"),
        }
    }

    /// The sentence an approval preview shows: "Repeats every Monday at 09:00".
    pub fn describe(&self, with_time: bool) -> String {
        let time = |at: &NaiveTime| {
            if with_time {
                format!(" at {}", at.format("%H:%M"))
            } else {
                String::new()
            }
        };
        match self {
            Self::Weekly { days, at } => {
                let names: Vec<&str> = days.iter().map(|d| d.long()).collect();
                let list = match names.as_slice() {
                    [] => String::new(),
                    [one] => (*one).to_string(),
                    [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
                };
                format!("Repeats every {list}{}", time(at))
            }
            Self::Once { .. } => self.label(with_time),
            other => {
                let label = other.label(with_time);
                match label.strip_prefix("Every ") {
                    Some(rest) => format!("Repeats every {rest}"),
                    None => label,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::{America::New_York, Asia::Kolkata, Europe::London};

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn hm(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn daily_times_across_dst_gaps_and_overlaps() {
        let daily = Schedule::Daily { at: hm(1, 30) };
        // London springs forward at 01:00 GMT on 29 March 2026: 01:30 does not exist and
        // falls on 02:00 BST (01:00 UTC); the next day is 01:30 BST (00:30 UTC).
        let gap = daily.next_after(&London, utc(2026, 3, 28, 12, 0)).unwrap();
        assert_eq!(gap, utc(2026, 3, 29, 1, 0));
        assert_eq!(
            daily.next_after(&London, gap).unwrap(),
            utc(2026, 3, 30, 0, 30)
        );
        // London falls back at 02:00 BST on 25 October 2026: 01:30 happens twice and
        // counts once, at its first (BST) instance.
        let first = daily.next_after(&London, utc(2026, 10, 24, 12, 0)).unwrap();
        assert_eq!(first, utc(2026, 10, 25, 0, 30));
        assert_eq!(
            daily.next_after(&London, first).unwrap(),
            utc(2026, 10, 26, 1, 30),
            "the second 01:30 of the night is not another occurrence"
        );
        // New York: 02:30 on 8 March 2026 does not exist (03:00 EDT, 07:00 UTC); 01:30 on
        // 1 November 2026 happens twice and counts once (EDT, 05:30 UTC).
        let ny = Schedule::Daily { at: hm(2, 30) };
        assert_eq!(
            ny.next_after(&New_York, utc(2026, 3, 7, 12, 0)).unwrap(),
            utc(2026, 3, 8, 7, 0)
        );
        let back = Schedule::Daily { at: hm(1, 30) };
        let once = back
            .next_after(&New_York, utc(2026, 10, 31, 12, 0))
            .unwrap();
        assert_eq!(once, utc(2026, 11, 1, 5, 30));
        assert_eq!(
            back.next_after(&New_York, once).unwrap(),
            utc(2026, 11, 2, 6, 30)
        );
        // Kolkata has no daylight saving: always +05:30.
        let kolkata = Schedule::Daily { at: hm(9, 0) };
        assert_eq!(
            kolkata
                .next_after(&Kolkata, utc(2026, 11, 23, 3, 30))
                .unwrap(),
            utc(2026, 11, 24, 3, 30)
        );
    }

    #[test]
    fn every_hours_counts_real_hours_across_dst() {
        let every = Schedule::EveryHours {
            hours: 2,
            from: utc(2026, 10, 24, 22, 0),
        };
        let (times, more) = every.occurrences_in(
            &London,
            utc(2026, 10, 24, 22, 0),
            utc(2026, 10, 25, 6, 0),
            10,
        );
        assert!(!more);
        assert_eq!(times.len(), 4);
        for pair in times.windows(2) {
            assert_eq!(pair[1] - pair[0], Duration::hours(2));
        }
        assert_eq!(
            every.next_after(&London, utc(2026, 10, 1, 0, 0)),
            Some(utc(2026, 10, 24, 22, 0)),
            "before its start, the start"
        );
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let weekdays = Schedule::Weekdays { at: hm(9, 0) };
        // Friday 27 November 2026, 09:00 in Kolkata, then Monday 30 November.
        assert_eq!(
            weekdays
                .next_after(&Kolkata, utc(2026, 11, 27, 3, 30))
                .unwrap(),
            utc(2026, 11, 30, 3, 30)
        );
    }

    #[test]
    fn weekly_on_chosen_days_in_order() {
        let weekly = Schedule::Weekly {
            days: vec![WeekDay::Thu, WeekDay::Mon, WeekDay::Thu],
            at: hm(17, 0),
        }
        .normalized();
        assert_eq!(
            weekly,
            Schedule::Weekly {
                days: vec![WeekDay::Mon, WeekDay::Thu],
                at: hm(17, 0)
            }
        );
        // Monday 23 November 2026, 17:30 in Kolkata: Thursday 26, then Monday 30.
        let first = weekly
            .next_after(&Kolkata, utc(2026, 11, 23, 12, 0))
            .unwrap();
        assert_eq!(first, utc(2026, 11, 26, 11, 30));
        assert_eq!(
            weekly.next_after(&Kolkata, first).unwrap(),
            utc(2026, 11, 30, 11, 30)
        );
    }

    #[test]
    fn monthly_day_31_falls_on_the_last_day_of_short_months() {
        let monthly = Schedule::Monthly {
            day: MonthDay::Day(31),
            at: hm(9, 0),
        };
        let jan = utc(2027, 1, 31, 3, 30);
        let feb = monthly.next_after(&Kolkata, jan).unwrap();
        assert_eq!(feb, utc(2027, 2, 28, 3, 30));
        assert_eq!(
            monthly.next_after(&Kolkata, feb).unwrap(),
            utc(2027, 3, 31, 3, 30)
        );
        let thirtieth = Schedule::Monthly {
            day: MonthDay::Day(30),
            at: hm(9, 0),
        };
        assert_eq!(
            thirtieth.next_after(&Utc, utc(2027, 4, 1, 0, 0)).unwrap(),
            utc(2027, 4, 30, 9, 0)
        );
    }

    #[test]
    fn monthly_last_day_in_leap_and_common_years() {
        let last = Schedule::Monthly {
            day: MonthDay::Last,
            at: hm(9, 0),
        };
        assert_eq!(
            last.next_after(&Utc, utc(2028, 2, 1, 0, 0)).unwrap(),
            utc(2028, 2, 29, 9, 0)
        );
        assert_eq!(
            last.next_after(&Utc, utc(2027, 2, 1, 0, 0)).unwrap(),
            utc(2027, 2, 28, 9, 0)
        );
        assert_eq!(
            last.next_after(&Utc, utc(2100, 2, 1, 0, 0)).unwrap(),
            utc(2100, 2, 28, 9, 0),
            "2100 is not a leap year"
        );
        let twenty_ninth = Schedule::Monthly {
            day: MonthDay::Day(29),
            at: hm(9, 0),
        };
        assert_eq!(
            twenty_ninth
                .next_after(&Utc, utc(2028, 2, 1, 0, 0))
                .unwrap(),
            utc(2028, 2, 29, 9, 0)
        );
    }

    #[test]
    fn occurrences_in_counts_a_three_day_gap() {
        let daily = Schedule::Daily { at: hm(9, 0) };
        let (times, more) = daily.occurrences_in(
            &Utc,
            utc(2026, 11, 20, 9, 0),
            utc(2026, 11, 23, 10, 0),
            1000,
        );
        assert_eq!(
            times,
            vec![
                utc(2026, 11, 21, 9, 0),
                utc(2026, 11, 22, 9, 0),
                utc(2026, 11, 23, 9, 0)
            ]
        );
        assert!(!more);
        let hourly = Schedule::EveryHours {
            hours: 1,
            from: utc(2026, 1, 1, 0, 0),
        };
        let (times, more) =
            hourly.occurrences_in(&Utc, utc(2026, 1, 1, 0, 0), utc(2027, 1, 1, 0, 0), 1000);
        assert_eq!(times.len(), 1000);
        assert!(more);
    }

    #[test]
    fn once_is_due_once() {
        let once = Schedule::Once {
            at: NaiveDate::from_ymd_opt(2026, 12, 1)
                .unwrap()
                .and_time(hm(9, 0)),
        };
        assert_eq!(
            once.next_after(&Utc, utc(2026, 11, 1, 0, 0)),
            Some(utc(2026, 12, 1, 9, 0))
        );
        assert_eq!(once.next_after(&Utc, utc(2026, 12, 1, 9, 0)), None);
    }

    #[test]
    fn schedules_round_trip_as_tagged_json() {
        let cases = [
            (
                Schedule::Daily { at: hm(9, 0) },
                serde_json::json!({ "kind": "daily", "at": "09:00" }),
            ),
            (
                Schedule::Weekdays { at: hm(8, 30) },
                serde_json::json!({ "kind": "weekdays", "at": "08:30" }),
            ),
            (
                Schedule::Weekly {
                    days: vec![WeekDay::Mon, WeekDay::Thu],
                    at: hm(17, 0),
                },
                serde_json::json!({ "kind": "weekly", "days": ["mon", "thu"], "at": "17:00" }),
            ),
            (
                Schedule::Monthly {
                    day: MonthDay::Day(24),
                    at: hm(9, 0),
                },
                serde_json::json!({ "kind": "monthly", "day": 24, "at": "09:00" }),
            ),
            (
                Schedule::Monthly {
                    day: MonthDay::Last,
                    at: hm(0, 0),
                },
                serde_json::json!({ "kind": "monthly", "day": "last", "at": "00:00" }),
            ),
            (
                Schedule::Once {
                    at: NaiveDate::from_ymd_opt(2026, 12, 1)
                        .unwrap()
                        .and_time(hm(9, 0)),
                },
                serde_json::json!({ "kind": "once", "at": "2026-12-01T09:00" }),
            ),
        ];
        for (schedule, json) in cases {
            assert_eq!(serde_json::to_value(&schedule).unwrap(), json);
            assert_eq!(serde_json::from_value::<Schedule>(json).unwrap(), schedule);
        }
        let every: Schedule = serde_json::from_value(serde_json::json!({
            "kind": "every_hours", "hours": 3, "from": "2026-11-23T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(
            every,
            Schedule::EveryHours {
                hours: 3,
                from: utc(2026, 11, 23, 0, 0)
            }
        );
        assert!(serde_json::from_value::<Schedule>(
            serde_json::json!({ "kind": "monthly", "day": "first", "at": "09:00" })
        )
        .is_err());
        assert!(serde_json::from_value::<Schedule>(
            serde_json::json!({ "kind": "daily", "at": "9am" })
        )
        .is_err());
    }

    #[test]
    fn invalid_schedules_are_refused() {
        let at = hm(9, 0);
        for bad in [
            Schedule::Weekly {
                days: Vec::new(),
                at,
            },
            Schedule::Monthly {
                day: MonthDay::Day(0),
                at,
            },
            Schedule::Monthly {
                day: MonthDay::Day(32),
                at,
            },
            Schedule::EveryHours {
                hours: 0,
                from: utc(2026, 1, 1, 0, 0),
            },
            Schedule::EveryHours {
                hours: 25,
                from: utc(2026, 1, 1, 0, 0),
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
            assert_eq!(bad.next_after(&Utc, utc(2026, 1, 1, 0, 0)), None);
        }
    }

    #[test]
    fn describe_reads_plainly() {
        let at = hm(9, 0);
        assert_eq!(
            Schedule::Weekly {
                days: vec![WeekDay::Mon],
                at
            }
            .describe(true),
            "Repeats every Monday at 09:00"
        );
        assert_eq!(
            Schedule::Weekly {
                days: vec![WeekDay::Mon, WeekDay::Wed, WeekDay::Fri],
                at
            }
            .describe(false),
            "Repeats every Monday, Wednesday and Friday"
        );
        assert_eq!(
            Schedule::Daily { at }.describe(true),
            "Repeats every day at 09:00"
        );
        assert_eq!(
            Schedule::Monthly {
                day: MonthDay::Day(22),
                at
            }
            .label(false),
            "Every month on the 22nd"
        );
        assert_eq!(
            Schedule::Monthly {
                day: MonthDay::Day(11),
                at
            }
            .label(false),
            "Every month on the 11th"
        );
        assert_eq!(
            Schedule::Monthly {
                day: MonthDay::Last,
                at
            }
            .describe(true),
            "Repeats every month on the last day at 09:00"
        );
        assert_eq!(
            local_key(&Kolkata, utc(2026, 11, 24, 11, 30)),
            "202611241700"
        );
    }
}
