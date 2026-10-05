//! A burn-rate estimate from the observations of the current window.
//!
//! It is an estimate and is worded as one: it assumes the pace seen between
//! the first and the last observation of the window continues. It says
//! nothing without two observations a minute apart.

/// One observation of one window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// When it was observed (Unix seconds).
    pub at: i64,
    /// How much was used then.
    pub used_percent: f64,
}

/// What the pace says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forecast {
    /// Fewer than two usable observations, or too close together.
    NotEnoughData,
    /// The limit is already reached.
    AlreadyOver,
    /// Nothing was used over the observed span.
    NoPace,
    /// At this pace the limit comes in this many seconds, before the reset.
    HitsIn(i64),
    /// At this pace the window resets first.
    SafeUntilReset,
}

/// The least time between the first and last observation worth a pace.
pub const MIN_SPAN: i64 = 60;

/// Estimates when `current` (the latest percentage) reaches 100 % at the pace
/// of `samples`, comparing with `resets_at`.
///
/// Only the observations after the window's last reset count: a sample whose
/// percentage is lower than the one before it starts a new window, and
/// samples at or after `resets_at` are ignored. The samples need not be
/// sorted.
pub fn forecast(samples: &[Sample], current: f64, now: i64, resets_at: Option<i64>) -> Forecast {
    if current >= 100.0 {
        return Forecast::AlreadyOver;
    }
    let mut ordered: Vec<Sample> = samples
        .iter()
        .copied()
        .filter(|s| resets_at.is_none_or(|r| s.at < r) && s.at <= now)
        .collect();
    ordered.sort_by_key(|s| s.at);
    // Keep the run after the last drop: a drop is a reset.
    let start = ordered
        .windows(2)
        .rposition(|pair| pair[1].used_percent + 0.5 < pair[0].used_percent)
        .map_or(0, |index| index + 1);
    let run = &ordered[start..];
    let (Some(first), Some(last)) = (run.first(), run.last()) else {
        return Forecast::NotEnoughData;
    };
    let span = last.at - first.at;
    if run.len() < 2 || span < MIN_SPAN {
        return Forecast::NotEnoughData;
    }
    let gained = last.used_percent - first.used_percent;
    if gained <= 0.0 {
        return Forecast::NoPace;
    }
    let per_second = gained / span as f64;
    let seconds = ((100.0 - current) / per_second).ceil() as i64;
    match resets_at {
        Some(reset) if now + seconds >= reset => Forecast::SafeUntilReset,
        _ => Forecast::HitsIn(seconds),
    }
}

impl Forecast {
    /// The sentence for the popover, or `None` when there is nothing to say.
    /// It always says it is an estimate.
    pub fn sentence(self) -> Option<String> {
        match self {
            Forecast::NotEnoughData | Forecast::NoPace => None,
            Forecast::AlreadyOver => Some("Limit reached.".into()),
            Forecast::HitsIn(seconds) => Some(format!(
                "At this pace: limit in ~{} (estimate).",
                crate::present::compact_duration(seconds)
            )),
            Forecast::SafeUntilReset => {
                Some("At this pace you will not hit the limit before the reset (estimate).".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn s(at: i64, used: f64) -> Sample {
        Sample {
            at,
            used_percent: used,
        }
    }

    #[test]
    fn nothing_is_said_without_two_observations() {
        assert_eq!(forecast(&[], 10.0, NOW, None), Forecast::NotEnoughData);
        assert_eq!(
            forecast(&[s(NOW, 10.0)], 10.0, NOW, None),
            Forecast::NotEnoughData
        );
    }

    #[test]
    fn two_observations_closer_than_a_minute_say_nothing() {
        let samples = [s(NOW - 30, 10.0), s(NOW, 20.0)];
        assert_eq!(forecast(&samples, 20.0, NOW, None), Forecast::NotEnoughData);
    }

    #[test]
    fn a_steady_pace_gives_the_time_to_the_limit() {
        // 10 points an hour, 60 left: six hours.
        let samples = [s(NOW - 3600, 30.0), s(NOW, 40.0)];
        assert_eq!(
            forecast(&samples, 40.0, NOW, None),
            Forecast::HitsIn(6 * 3600)
        );
    }

    #[test]
    fn a_pace_that_outlasts_the_window_is_safe() {
        let samples = [s(NOW - 3600, 30.0), s(NOW, 40.0)];
        assert_eq!(
            forecast(&samples, 40.0, NOW, Some(NOW + 3600)),
            Forecast::SafeUntilReset
        );
    }

    #[test]
    fn a_pace_that_hits_before_the_reset_says_when() {
        let samples = [s(NOW - 3600, 30.0), s(NOW, 70.0)];
        assert_eq!(
            forecast(&samples, 70.0, NOW, Some(NOW + 5 * 3600)),
            Forecast::HitsIn(2700)
        );
    }

    #[test]
    fn no_progress_is_no_pace() {
        let samples = [s(NOW - 3600, 40.0), s(NOW, 40.0)];
        assert_eq!(forecast(&samples, 40.0, NOW, None), Forecast::NoPace);
    }

    #[test]
    fn a_window_at_its_limit_is_already_over() {
        let samples = [s(NOW - 3600, 90.0), s(NOW, 100.0)];
        assert_eq!(forecast(&samples, 100.0, NOW, None), Forecast::AlreadyOver);
    }

    #[test]
    fn observations_before_a_reset_do_not_count() {
        // 90 -> 5 is a reset; only 5 -> 10 is the current window.
        let samples = [
            s(NOW - 7200, 80.0),
            s(NOW - 5400, 90.0),
            s(NOW - 3600, 5.0),
            s(NOW, 10.0),
        ];
        assert_eq!(
            forecast(&samples, 10.0, NOW, None),
            Forecast::HitsIn(((90.0_f64) / (5.0 / 3600.0)).ceil() as i64)
        );
    }

    #[test]
    fn observations_after_the_reset_time_are_ignored() {
        let samples = [s(NOW - 3600, 30.0), s(NOW, 40.0), s(NOW + 10, 99.0)];
        assert_eq!(
            forecast(&samples, 40.0, NOW, Some(NOW + 100)),
            Forecast::SafeUntilReset
        );
    }

    #[test]
    fn unsorted_samples_are_sorted() {
        let samples = [s(NOW, 40.0), s(NOW - 3600, 30.0)];
        assert_eq!(
            forecast(&samples, 40.0, NOW, None),
            Forecast::HitsIn(6 * 3600)
        );
    }

    #[test]
    fn the_sentence_says_it_is_an_estimate() {
        assert!(Forecast::HitsIn(4200)
            .sentence()
            .unwrap()
            .contains("limit in ~1h 10m (estimate)"));
        assert!(Forecast::SafeUntilReset
            .sentence()
            .unwrap()
            .contains("estimate"));
        assert_eq!(Forecast::NotEnoughData.sentence(), None);
        assert_eq!(Forecast::NoPace.sentence(), None);
    }
}
