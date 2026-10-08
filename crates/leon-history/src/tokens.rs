//! Token counts as each agent's transcript states them, in one vocabulary.
//!
//! Every function here is pure: a JSON value in, [`TokenCounts`] out. The
//! parsers decide which record to ask about; this module knows only what the
//! numbers inside it mean, because the three agents do not agree:
//!
//! * **Claude Code** states `input_tokens` (fresh input only),
//!   `cache_read_input_tokens`, `cache_creation_input_tokens` and, when it
//!   knows, how much of the cache writes were the five-minute and the
//!   one-hour kind (`cache_creation`). `output_tokens` includes thinking.
//! * **Codex** states running totals. `input_tokens` *includes* the cached
//!   part (`cached_input_tokens`) and the cache writes, and `output_tokens`
//!   includes the reasoning, so the fresh input is what is left of
//!   `input_tokens` and nothing is added to the output. A `token_count` record
//!   can be written again with the same totals, and a record's totals are
//!   what matters, so [`CodexMeter`] turns them into what each call used.
//! * **opencode** states `input` (fresh), `output`, `reasoning` (billed as
//!   output, so added to it) and `cache.read` / `cache.write`.
//!
//! Anything missing, negative or not a number counts as zero.

use leon_core::TokenCounts;
use serde_json::Value;

/// The number at `path` inside `value`, zero when it is not a non-negative
/// whole number.
fn number(value: &Value, path: &[&str]) -> u64 {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// What one Claude Code reply used, from its `message.usage`.
pub(crate) fn claude_counts(usage: &Value) -> TokenCounts {
    let written = number(usage, &["cache_creation_input_tokens"]);
    let hour = number(usage, &["cache_creation", "ephemeral_1h_input_tokens"]).min(written);
    TokenCounts {
        input: number(usage, &["input_tokens"]),
        output: number(usage, &["output_tokens"]),
        cache_read: number(usage, &["cache_read_input_tokens"]),
        cache_write: written - hour,
        cache_write_1h: hour,
    }
}

/// What one opencode reply used, from the `tokens` of its message.
pub(crate) fn opencode_counts(tokens: &Value) -> TokenCounts {
    TokenCounts {
        input: number(tokens, &["input"]),
        output: number(tokens, &["output"]).saturating_add(number(tokens, &["reasoning"])),
        cache_read: number(tokens, &["cache", "read"]),
        cache_write: number(tokens, &["cache", "write"]),
        cache_write_1h: 0,
    }
}

/// A Codex usage object as written: input including the cached part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CodexRaw {
    input: u64,
    cached: u64,
    written: u64,
    output: u64,
}

impl CodexRaw {
    /// Reads `total_token_usage` or `last_token_usage`; `None` when the value
    /// is not an object.
    pub(crate) fn of(value: &Value) -> Option<Self> {
        value.is_object().then(|| Self {
            input: number(value, &["input_tokens"]),
            cached: number(value, &["cached_input_tokens"]),
            written: number(value, &["cache_write_input_tokens"]),
            output: number(value, &["output_tokens"]),
        })
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether no field is below the one in `earlier`.
    fn covers(&self, earlier: &Self) -> bool {
        self.input >= earlier.input
            && self.cached >= earlier.cached
            && self.written >= earlier.written
            && self.output >= earlier.output
    }

    fn minus(&self, earlier: &Self) -> Self {
        Self {
            input: self.input - earlier.input,
            cached: self.cached - earlier.cached,
            written: self.written - earlier.written,
            output: self.output - earlier.output,
        }
    }

    fn counts(&self) -> TokenCounts {
        TokenCounts {
            input: self.input.saturating_sub(self.cached + self.written),
            output: self.output,
            cache_read: self.cached,
            cache_write: self.written,
            cache_write_1h: 0,
        }
    }
}

/// Follows a Codex session's running totals and says what each `token_count`
/// record added.
#[derive(Debug, Default)]
pub(crate) struct CodexMeter {
    seen: CodexRaw,
}

impl CodexMeter {
    /// What the record added to the session. A record whose totals did not
    /// move adds nothing (Codex writes some twice). When the totals went down
    /// (a fresh count after the earlier ones), the record's own last call is
    /// what it adds; a record with no totals adds its last call.
    pub(crate) fn observe(
        &mut self,
        total: Option<CodexRaw>,
        last: Option<CodexRaw>,
    ) -> TokenCounts {
        let added = match total {
            Some(total) => {
                let added = if total.covers(&self.seen) {
                    total.minus(&self.seen)
                } else {
                    last.unwrap_or_default()
                };
                self.seen = total;
                added
            }
            None => last.unwrap_or_default(),
        };
        if added.is_empty() {
            TokenCounts::default()
        } else {
            added.counts()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A Claude Code reply's usage as the transcript records it.
    fn claude_usage() -> Value {
        json!({
            "input_tokens": 2, "cache_creation_input_tokens": 14227,
            "cache_read_input_tokens": 30516, "output_tokens": 54,
            "output_tokens_details": {"thinking_tokens": 0},
            "server_tool_use": {"web_search_requests": 0, "web_fetch_requests": 0},
            "service_tier": "standard",
            "cache_creation": {"ephemeral_1h_input_tokens": 14227, "ephemeral_5m_input_tokens": 0},
            "inference_geo": "not_available", "speed": "standard"
        })
    }

    #[test]
    fn a_claude_reply_keeps_the_hour_cache_writes_apart() {
        let counts = claude_counts(&claude_usage());
        assert_eq!(
            counts,
            TokenCounts {
                input: 2,
                output: 54,
                cache_read: 30516,
                cache_write: 0,
                cache_write_1h: 14227,
            }
        );
        // Without the split every write is the short kind.
        let older =
            json!({"input_tokens": 5, "cache_creation_input_tokens": 90, "output_tokens": 1});
        assert_eq!(claude_counts(&older).cache_write, 90);
        assert_eq!(claude_counts(&older).cache_write_1h, 0);
        // A split larger than the total cannot make a negative count.
        let odd = json!({"cache_creation_input_tokens": 10,
            "cache_creation": {"ephemeral_1h_input_tokens": 99}});
        assert_eq!(claude_counts(&odd).cache_write_1h, 10);
        assert_eq!(claude_counts(&odd).cache_write, 0);
    }

    #[test]
    fn odd_claude_usage_counts_as_zero() {
        assert!(claude_counts(&json!("none")).is_empty());
        assert!(claude_counts(&json!({"input_tokens": -4, "output_tokens": "9"})).is_empty());
        assert!(claude_counts(&json!(null)).is_empty());
    }

    #[test]
    fn an_opencode_reply_bills_reasoning_as_output() {
        let tokens = json!({"input": 798, "output": 146, "reasoning": 52,
            "cache": {"read": 12416, "write": 0}});
        assert_eq!(
            opencode_counts(&tokens),
            TokenCounts {
                input: 798,
                output: 198,
                cache_read: 12416,
                cache_write: 0,
                cache_write_1h: 0
            }
        );
        assert!(opencode_counts(&json!({"input": 0, "output": 0})).is_empty());
    }

    fn raw(input: u64, cached: u64, output: u64) -> CodexRaw {
        CodexRaw::of(&json!({
            "input_tokens": input, "cached_input_tokens": cached,
            "cache_write_input_tokens": 0, "output_tokens": output,
            "reasoning_output_tokens": 77, "total_tokens": input + output
        }))
        .unwrap()
    }

    #[test]
    fn codex_input_is_what_is_left_after_the_cached_part_and_output_keeps_its_reasoning() {
        let mut meter = CodexMeter::default();
        let first = meter.observe(Some(raw(23881, 11520, 326)), Some(raw(23881, 11520, 326)));
        assert_eq!(
            first,
            TokenCounts {
                input: 12361,
                output: 326,
                cache_read: 11520,
                cache_write: 0,
                cache_write_1h: 0
            }
        );
    }

    #[test]
    fn codex_totals_are_turned_into_what_each_record_added() {
        // Recorded totals of one session, three records in a row.
        let totals = [
            raw(23881, 11520, 326),
            raw(54578, 23040, 877),
            raw(92755, 53504, 1042),
        ];
        let mut meter = CodexMeter::default();
        let mut sum = TokenCounts::default();
        for total in totals {
            sum.add(&meter.observe(Some(total), None));
        }
        // What the last total says, split the same way.
        assert_eq!(sum, totals[2].counts());
    }

    #[test]
    fn a_record_written_twice_is_counted_once() {
        let mut meter = CodexMeter::default();
        let total = raw(100, 40, 10);
        assert!(!meter.observe(Some(total), Some(total)).is_empty());
        assert!(meter.observe(Some(total), Some(total)).is_empty());
    }

    #[test]
    fn totals_that_go_down_fall_back_to_the_last_call() {
        let mut meter = CodexMeter::default();
        meter.observe(Some(raw(1000, 0, 100)), None);
        let after = meter.observe(Some(raw(50, 0, 5)), Some(raw(50, 0, 5)));
        assert_eq!((after.input, after.output), (50, 5));
        // And the next record is measured against the new totals.
        let next = meter.observe(Some(raw(80, 0, 9)), None);
        assert_eq!((next.input, next.output), (30, 4));
    }

    #[test]
    fn a_record_without_totals_adds_its_last_call_and_one_without_numbers_nothing() {
        let mut meter = CodexMeter::default();
        let only_last = meter.observe(None, Some(raw(10, 0, 2)));
        assert_eq!((only_last.input, only_last.output), (10, 2));
        assert!(meter.observe(None, None).is_empty());
        assert!(CodexRaw::of(&json!(null)).is_none());
    }

    #[test]
    fn cache_writes_are_not_fresh_input() {
        let with_writes = CodexRaw::of(&json!({
            "input_tokens": 100, "cached_input_tokens": 30,
            "cache_write_input_tokens": 20, "output_tokens": 5
        }))
        .unwrap();
        let counts = CodexMeter::default().observe(Some(with_writes), None);
        assert_eq!(
            (counts.input, counts.cache_read, counts.cache_write),
            (50, 30, 20)
        );
    }
}
