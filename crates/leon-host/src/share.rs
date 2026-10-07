//! What this installation's own Leon lets paired devices read of itself.
//!
//! A paired device already holds a full terminal as this computer's user, so
//! sharing what Leon knows about this machine adds no rights. It does add
//! convenience: the device's own Leon can show this machine's projects and
//! history sessions in its sidebar, and a person working from a laptop sees
//! what there is to work on without typing a path.
//!
//! The host application provides a [`ShareSource`] (usually one that reads
//! this process's own store); a headless `leon host` provides none and what
//! the share requests answer is nothing at all. Every method is called with
//! the host's own machine in mind: the machine Leon runs on.

use leon_wire::{SharedEntry, SharedProject, SharedSession, SharedTranscript};

/// How many transcript bytes one `ShareTranscriptData` may carry, counted as
/// its entries' text: a near-frame-sized reply is a reply that may fail to be
/// sent, and a client waiting on it a client that hangs. The first entries of
/// the transcript go out, the rest wait for nothing — the sender's count
/// still says how many the session holds.
pub const TRANSCRIPT_BUDGET_BYTES: usize = 6 * 1024 * 1024;

/// The one seam between the protocol and this installation's own data.
pub trait ShareSource: Send + Sync {
    /// The projects of this machine, in the order Leon shows them.
    fn projects(&self) -> Vec<SharedProject>;

    /// The history sessions of this machine, newest first. The host cuts the
    /// list at [`SHARE_SESSIONS_LIMIT`](leon_wire::SHARE_SESSIONS_LIMIT)
    /// itself, so a source may return more.
    fn sessions(&self) -> Vec<SharedSession>;

    /// The transcript of the session `(agent, external_id)` names, in order;
    /// `None` when no such session is held (it may have been removed).
    fn transcript(&self, agent: &str, external_id: &str) -> Option<SharedTranscript>;
}

/// Keeps the first entries of a transcript within `budget` bytes of text,
/// never empty when the transcript was not.
pub fn bound_transcript(messages: &[SharedEntry], budget: usize) -> Vec<SharedEntry> {
    let mut used = 0usize;
    let mut count = 0usize;
    for entry in messages {
        used = used.saturating_add(entry.text.len().saturating_add(16));
        if used > budget && count > 0 {
            break;
        }
        count += 1;
    }
    Vec::from(&messages[..count])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> SharedEntry {
        SharedEntry {
            role: "user".into(),
            text: text.into(),
            at_ms: 0,
        }
    }

    #[test]
    fn a_transcript_within_its_budget_is_whole() {
        let messages = vec![entry("short"), entry("also short")];
        let kept = bound_transcript(&messages, 128);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn a_transcript_over_its_budget_is_a_prefix_never_empty() {
        let messages = vec![entry("a"), entry("b"), entry("c")];
        let kept = bound_transcript(&messages, 32);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "a");
        // One entry larger than everything: it is kept alone, never dropped.
        assert_eq!(bound_transcript(&messages, 1).len(), 1);
        assert_eq!(bound_transcript(&messages, 1)[0].text, "a");
    }
}
