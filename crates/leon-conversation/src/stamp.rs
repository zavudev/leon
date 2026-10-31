//! When a message was said, as a person reads it next to the message:
//! "14:32" for today, "Mar 3, 14:32" for another day of the year, and with
//! the year for another one.
//!
//! Pure: seconds since the epoch and the offset of the local time zone in,
//! words out.

use chrono::DateTime;

const DAY: i64 = 86_400;

/// `at` as the time of a message, when `now` is the current time; both in
/// seconds since the epoch, `offset` the seconds the local time is ahead of
/// UTC.
pub fn label(at: i64, now: i64, offset: i32) -> String {
    let shifted = at + i64::from(offset);
    let Some(time) = DateTime::from_timestamp(shifted, 0) else {
        return String::new();
    };
    let today = (now + i64::from(offset)).div_euclid(DAY);
    if shifted.div_euclid(DAY) == today {
        return time.format("%H:%M").to_string();
    }
    let year = DateTime::from_timestamp(now + i64::from(offset), 0)
        .map(|now| now.format("%Y").to_string());
    if year.as_deref() == Some(time.format("%Y").to_string().as_str()) {
        time.format("%b %-d, %H:%M").to_string()
    } else {
        time.format("%b %-d %Y, %H:%M").to_string()
    }
}

/// The offset of this computer's time zone at `at`, in seconds.
pub fn local_offset(at: i64) -> i32 {
    use chrono::{Local, Offset, TimeZone};
    Local
        .timestamp_opt(at, 0)
        .single()
        .map_or(0, |time| time.offset().fix().local_minus_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-03-03 14:32:10 UTC.
    const AT: i64 = 1_772_548_330;

    #[test]
    fn a_message_of_today_is_its_time_of_day() {
        assert_eq!(label(AT, AT + 60, 0), "14:32");
        assert_eq!(label(AT, AT + 3 * 3600, 0), "14:32");
    }

    #[test]
    fn the_time_of_day_is_the_local_one() {
        // Two hours ahead of UTC: 16:32, and a message of 23:30 UTC is of
        // the next day there.
        assert_eq!(label(AT, AT, 2 * 3600), "16:32");
        let late = AT + 9 * 3600; // 23:32 UTC
        assert_eq!(label(late, late + 1800, 2 * 3600), "01:32");
        // And for someone at UTC it is the evening before.
        assert_eq!(label(late, late + 1800, 0), "Mar 3, 23:32");
    }

    #[test]
    fn a_message_of_another_day_says_the_date() {
        assert_eq!(label(AT, AT + DAY, 0), "Mar 3, 14:32");
        assert_eq!(label(AT, AT + 40 * DAY, 0), "Mar 3, 14:32");
    }

    #[test]
    fn a_message_of_another_year_says_the_year_too() {
        assert_eq!(label(AT, AT + 400 * DAY, 0), "Mar 3 2026, 14:32");
    }

    #[test]
    fn a_time_that_cannot_be_read_is_nothing() {
        assert_eq!(label(i64::MAX, 0, 0), "");
    }
}
