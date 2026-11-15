//! Which of the phone's calendars Shodh may touch. Calendar sync is opt-in:
//! the user picks the calendars, and Shodh reads only those and its own
//! ("Shodh"), and writes only to them. Every other calendar is never read, so
//! its events never leave the phone.
//!
//! Whether events of the chosen calendars go to the computer at all is a
//! second choice, "Let Shodh use my calendar" ([`CalendarChoice::use_with_shodh`]):
//! without it they are shown on the phone only, never sent, so they are never
//! indexed and the assistant never sees them.

use serde::{Deserialize, Serialize};

/// The user's calendar choices, kept sealed on the phone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarChoice {
    /// Calendar sync is on.
    pub enabled: bool,
    /// The phone calendars the user chose.
    pub chosen: Vec<i64>,
    /// Shodh's own calendar on the phone, once made.
    pub shodh_calendar: Option<i64>,
    /// "Let Shodh use my calendar": events of the chosen calendars go to the
    /// computer (indexed there, and the assistant may read them).
    pub use_with_shodh: bool,
}

impl CalendarChoice {
    /// The calendars Shodh may read and write: none while sync is off; else
    /// the chosen ones and its own.
    pub fn reachable(&self) -> Vec<i64> {
        if !self.enabled {
            return Vec::new();
        }
        let mut ids = self.chosen.clone();
        if let Some(own) = self.shodh_calendar {
            ids.push(own);
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Whether Shodh may read or write calendar `id`.
    pub fn may_touch(&self, id: i64) -> bool {
        self.reachable().contains(&id)
    }

    /// Of `requested`, only the calendars Shodh may read.
    pub fn readable(&self, requested: &[i64]) -> Vec<i64> {
        let reachable = self.reachable();
        let mut ids: Vec<i64> = requested
            .iter()
            .copied()
            .filter(|id| reachable.contains(id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Whether events of the phone's own calendars may go to the computer.
    pub fn sends_phone_events(&self) -> bool {
        self.enabled && self.use_with_shodh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unselected_calendars_never_leave_the_phone() {
        let off = CalendarChoice {
            enabled: false,
            chosen: vec![3, 4],
            shodh_calendar: Some(9),
            use_with_shodh: true,
        };
        assert!(off.reachable().is_empty(), "sync off: nothing is read");
        assert!(!off.sends_phone_events());

        let on = CalendarChoice {
            enabled: true,
            ..off.clone()
        };
        assert_eq!(on.readable(&[1, 2, 3, 4, 9, 3]), vec![3, 4, 9]);
        assert!(on.may_touch(9) && on.may_touch(3));
        assert!(!on.may_touch(1), "a calendar the user did not choose");
        assert!(on.sends_phone_events());

        let private = CalendarChoice {
            use_with_shodh: false,
            ..on
        };
        assert!(!private.sends_phone_events(), "shown on the phone only");
        assert_eq!(private.readable(&[3]), vec![3]);
    }

    #[test]
    fn the_choice_reads_as_the_phone_saved_it() {
        let saved: CalendarChoice =
            serde_json::from_str(r#"{"enabled":true,"chosen":[5]}"#).unwrap();
        assert_eq!(saved.shodh_calendar, None);
        assert!(!saved.use_with_shodh, "consent is off unless given");
    }
}
