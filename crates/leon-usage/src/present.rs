//! How a reading is worded: thresholds and durations.
//!
//! Pure functions, so the bar, the popover and `--diagnose usage` all say the
//! same thing and the words are tested once.

use crate::model::{DAY, HOUR, MINUTE};

/// How close to its limit a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Plenty left.
    Normal,
    /// Getting close.
    Warning,
    /// At or near the limit.
    Critical,
}

impl Level {
    /// A glyph that says the level without colour: nothing, `!` or `!!`.
    pub fn glyph(self) -> &'static str {
        match self {
            Level::Normal => "",
            Level::Warning => "!",
            Level::Critical => "!!",
        }
    }

    /// One word for a tooltip.
    pub fn word(self) -> &'static str {
        match self {
            Level::Normal => "ok",
            Level::Warning => "high",
            Level::Critical => "near the limit",
        }
    }
}

/// The percentages at which a window becomes a warning and critical.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// From here on a window is a warning.
    pub warning: f64,
    /// From here on a window is critical.
    pub critical: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warning: 60.0,
            critical: 80.0,
        }
    }
}

impl Thresholds {
    /// The level of a window at `percent`.
    pub fn classify(self, percent: f64) -> Level {
        if percent >= self.critical {
            Level::Critical
        } else if percent >= self.warning {
            Level::Warning
        } else {
            Level::Normal
        }
    }
}

/// A duration in seconds as `2h 29m`, `1d 11h`, `12m` or `<1m`. Negative
/// durations read as `<1m`.
pub fn compact_duration(seconds: i64) -> String {
    if seconds < MINUTE {
        return "<1m".into();
    }
    if seconds >= DAY {
        let days = seconds / DAY;
        let hours = (seconds % DAY) / HOUR;
        return if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        };
    }
    if seconds >= HOUR {
        let hours = seconds / HOUR;
        let minutes = (seconds % HOUR) / MINUTE;
        return if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        };
    }
    format!("{}m", seconds / MINUTE)
}

/// How long ago something was, for the freshness line: `just now`, `3 min
/// ago`, `2 h ago`, `4 d ago`.
pub fn ago(seconds: i64) -> String {
    match seconds {
        s if s < 45 => "just now".into(),
        s if s < HOUR => format!("{} min ago", (s + 30) / MINUTE),
        s if s < DAY => format!("{} h ago", s / HOUR),
        s => format!("{} d ago", s / DAY),
    }
}

/// A used percentage as the whole number every surface shows: clamped to 0..100
/// and rounded half away from zero (12.5 reads 13), as Orca does. Anything that
/// is not a number reads 0.
pub fn percent_round(percent: f64) -> i64 {
    if percent.is_finite() {
        percent.clamp(0.0, 100.0).round() as i64
    } else {
        0
    }
}

/// A percentage as a fixed-width figure: `  7%`, ` 38%`, `100%`.
pub fn percent_fixed(percent: f64) -> String {
    format!("{:>3}%", percent_round(percent))
}

/// Whether a figure says how much is used or how much is left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PercentDisplay {
    /// `N% used`.
    #[default]
    Used,
    /// `N% left`, computed as 100 minus the rounded used figure.
    Remaining,
}

impl PercentDisplay {
    /// The setting's value: `used` or `remaining`; anything else is `used`.
    pub fn parse(value: &str) -> Self {
        if value == "remaining" {
            Self::Remaining
        } else {
            Self::Used
        }
    }

    /// The number shown for a used percentage. The used figure is rounded
    /// first, so `20.5` reads 21 used and 79 left, never 80 left.
    pub fn number(self, used: f64) -> i64 {
        match self {
            Self::Used => percent_round(used),
            Self::Remaining => 100 - percent_round(used),
        }
    }

    /// `38% used` or `62% left`.
    pub fn label(self, used: f64) -> String {
        let word = match self {
            Self::Used => "used",
            Self::Remaining => "left",
        };
        format!("{}% {word}", self.number(used))
    }

    /// The figure with the fixed width of [`percent_fixed`]: ` 38%`, ` 62%`.
    pub fn fixed(self, used: f64) -> String {
        format!("{:>3}%", self.number(used))
    }
}

/// The time until a reset the way Orca words it, floored to whole units:
/// `47m`, `3h 54m`, `6d 7h`, and `now` when it is not ahead. Used for the
/// labels of the bar's windows, so they count down.
pub fn countdown(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".into();
    }
    let minutes = seconds / MINUTE;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    if hours >= 24 {
        let days = hours / 24;
        let hours = hours % 24;
        return if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        };
    }
    if rest > 0 {
        format!("{hours}h {rest}m")
    } else {
        format!("{hours}h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_change_at_the_thresholds() {
        let t = Thresholds::default();
        assert_eq!(t.classify(0.0), Level::Normal);
        assert_eq!(t.classify(59.0), Level::Normal);
        assert_eq!(t.classify(59.9), Level::Normal);
        assert_eq!(t.classify(60.0), Level::Warning);
        assert_eq!(t.classify(79.0), Level::Warning);
        assert_eq!(t.classify(80.0), Level::Critical);
        assert_eq!(t.classify(100.0), Level::Critical);
    }

    #[test]
    fn a_level_has_a_glyph_so_colour_is_never_alone() {
        assert_eq!(Level::Normal.glyph(), "");
        assert_eq!(Level::Warning.glyph(), "!");
        assert_eq!(Level::Critical.glyph(), "!!");
    }

    #[test]
    fn durations_are_compact() {
        assert_eq!(compact_duration(-5), "<1m");
        assert_eq!(compact_duration(59), "<1m");
        assert_eq!(compact_duration(60), "1m");
        assert_eq!(compact_duration(12 * 60 + 40), "12m");
        assert_eq!(compact_duration(2 * HOUR + 29 * MINUTE), "2h 29m");
        assert_eq!(compact_duration(3 * HOUR), "3h");
        assert_eq!(compact_duration(DAY + 11 * HOUR + 5 * MINUTE), "1d 11h");
        assert_eq!(compact_duration(2 * DAY), "2d");
    }

    #[test]
    fn ages_are_rounded_to_a_unit() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(180), "3 min ago");
        assert_eq!(ago(2 * HOUR + 100), "2 h ago");
        assert_eq!(ago(4 * DAY), "4 d ago");
    }

    #[test]
    fn figures_have_a_fixed_width() {
        assert_eq!(percent_fixed(7.2), "  7%");
        assert_eq!(percent_fixed(38.0), " 38%");
        assert_eq!(percent_fixed(100.0), "100%");
        assert_eq!(percent_fixed(140.0), "100%");
        assert_eq!(percent_fixed(-3.0), "  0%");
    }

    #[test]
    fn percentages_round_half_away_from_zero_and_clamp() {
        for (used, shown) in [
            (12.5, 13),
            (12.4, 12),
            (0.5, 1),
            (99.5, 100),
            (100.4, 100),
            (-3.0, 0),
            (f64::NAN, 0),
        ] {
            assert_eq!(percent_round(used), shown, "{used}");
        }
        assert_eq!(percent_fixed(12.5), " 13%");
    }

    #[test]
    fn remaining_is_the_complement_of_the_rounded_used_figure() {
        assert_eq!(PercentDisplay::Used.label(20.5), "21% used");
        assert_eq!(PercentDisplay::Remaining.label(20.5), "79% left");
        assert_eq!(PercentDisplay::Remaining.label(0.0), "100% left");
        assert_eq!(PercentDisplay::Remaining.label(140.0), "0% left");
        assert_eq!(PercentDisplay::Remaining.fixed(62.0), " 38%");
        assert_eq!(
            PercentDisplay::parse("remaining"),
            PercentDisplay::Remaining
        );
        assert_eq!(PercentDisplay::parse("anything"), PercentDisplay::Used);
    }

    #[test]
    fn countdowns_floor_like_orca() {
        for (seconds, text) in [
            (-5, "now"),
            (0, "now"),
            (59, "0m"),
            (47 * MINUTE + 59, "47m"),
            (3 * HOUR + 54 * MINUTE + 30, "3h 54m"),
            (2 * HOUR, "2h"),
            (6 * DAY + 7 * HOUR + 59 * MINUTE, "6d 7h"),
            (DAY + 11 * HOUR, "1d 11h"),
            (2 * DAY, "2d"),
        ] {
            assert_eq!(countdown(seconds), text, "{seconds}");
        }
    }
}
