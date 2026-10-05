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
            warning: 75.0,
            critical: 90.0,
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

/// A percentage as a fixed-width figure: `  7%`, ` 38%`, `100%`.
pub fn percent_fixed(percent: f64) -> String {
    format!("{:>3}%", percent.round().clamp(0.0, 100.0) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_change_at_the_thresholds() {
        let t = Thresholds::default();
        assert_eq!(t.classify(0.0), Level::Normal);
        assert_eq!(t.classify(74.9), Level::Normal);
        assert_eq!(t.classify(75.0), Level::Warning);
        assert_eq!(t.classify(89.9), Level::Warning);
        assert_eq!(t.classify(90.0), Level::Critical);
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
}
