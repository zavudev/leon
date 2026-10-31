//! Numbers as a person reads them: a count of tokens, a span of seconds, a
//! speed. One spelling, shared by the conversation's foot and by King's
//! report line, so the same figure never reads two ways.
//!
//! Pure: numbers in, text out.

/// A count of tokens, short: `950`, `12.3k`, `1.2M`.
pub fn count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// A span of seconds, short: `45s`, `12m`, `3h10m`, `2d4h`.
pub fn span(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d{}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

/// Tokens written per second, over the whole turn; `None` when nothing was
/// written or the turn's length is not known.
pub fn per_second(output: u64, duration_ms: u64) -> Option<f64> {
    (duration_ms > 0 && output > 0).then(|| output as f64 * 1000.0 / duration_ms as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_spans_read_short() {
        assert_eq!(count(950), "950");
        assert_eq!(count(12_345), "12.3k");
        assert_eq!(count(1_234_567), "1.2M");
        assert_eq!(span(45), "45s");
        assert_eq!(span(720), "12m");
        assert_eq!(span(11_400), "3h10m");
        assert_eq!(span(187_200), "2d4h");
        assert_eq!(span(-5), "0s");
    }

    #[test]
    fn the_edges_of_a_count_do_not_change_unit_early() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1.0k");
        assert_eq!(count(999_999), "1000.0k");
        assert_eq!(count(1_000_000), "1.0M");
    }

    #[test]
    fn a_speed_needs_something_written_over_some_time() {
        assert_eq!(
            per_second(3_100, 50_000).map(|rate| rate.round()),
            Some(62.0)
        );
        assert_eq!(per_second(0, 50_000), None);
        assert_eq!(per_second(3_100, 0), None);
    }
}
