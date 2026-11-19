use super::search::{civil_date, day_number};
use super::types::MessageDate;

// The times a conversation can be snoozed until.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnoozePreset {
    LaterToday,
    Tomorrow,
    ThisWeekend,
    NextWeek,
}

impl SnoozePreset {
    pub(crate) const ALL: [SnoozePreset; 4] = [
        SnoozePreset::LaterToday,
        SnoozePreset::Tomorrow,
        SnoozePreset::ThisWeekend,
        SnoozePreset::NextWeek,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            SnoozePreset::LaterToday => "In 3 hours",
            SnoozePreset::Tomorrow => "Tomorrow",
            SnoozePreset::ThisWeekend => "This weekend",
            SnoozePreset::NextWeek => "Next week",
        }
    }

    pub(crate) fn until(self, now: MessageDate) -> MessageDate {
        let today = day_number(now.year, now.month, now.day);
        // 1970-01-01 was a Thursday; 0 is Sunday.
        let weekday = (today + 4).rem_euclid(7);
        let (day, minute) = match self {
            SnoozePreset::LaterToday => {
                let later = i64::from(now.minute) + 3 * 60;
                (today + later.div_euclid(1440), later.rem_euclid(1440))
            }
            SnoozePreset::Tomorrow => (today + 1, 8 * 60),
            SnoozePreset::ThisWeekend => {
                let ahead = (6 - weekday).rem_euclid(7);
                (today + if ahead == 0 { 7 } else { ahead }, 9 * 60)
            }
            SnoozePreset::NextWeek => {
                let ahead = (1 - weekday).rem_euclid(7);
                (today + if ahead == 0 { 7 } else { ahead }, 8 * 60)
            }
        };
        at(day, minute)
    }
}

fn at(day: i64, minute: i64) -> MessageDate {
    let (year, month, day) = civil_date(day);
    MessageDate {
        year: u32::try_from(year).unwrap_or(0),
        month,
        day,
        minute: u32::try_from(minute).unwrap_or(0),
    }
}

// Minutes from `now` until `until`, negative once it has passed.
pub(crate) fn minutes_until(now: MessageDate, until: MessageDate) -> i64 {
    let stamp = |d: MessageDate| day_number(d.year, d.month, d.day) * 1440 + i64::from(d.minute);
    stamp(until) - stamp(now)
}
