//! Updates for Leon, from the releases of its own repository on GitHub and
//! from nowhere else.
//!
//! No window, no manifest of our own, no key of our own: the source of truth
//! is `https://github.com/zavudev/leon/releases`. A check asks the API for
//! the latest release (with its ETag, so asking again costs nothing); the file
//! for this platform is chosen by name; it is downloaded from the release's own
//! address and held to the `SHA256SUMS` of the same release; on macOS and
//! Windows a signed install is replaced only by a build with the same
//! signer; it is put in place with the old version kept beside it, and put
//! back if the new one does not come up. `docs/UPDATES.md` says what that
//! protects against and what it does not.
//!
//! The pieces, in the order an update goes through them:
//!
//! * [`version`]: which tags count and which version is newer.
//! * [`release`]: the API's answer, and the file of this platform in it.
//! * [`http`] and [`download`]: the system's `curl` behind a trait, host
//!   allow-list, redirects by hand, resumable download with progress.
//! * [`checksums`] and [`trust`]: the checksum file, and the signature rules.
//! * [`package`] and [`tools`]: taking the program out of the archive; the
//!   system tools (disk images, code signatures) behind a trait.
//! * [`install`] and [`launch`]: where Leon is installed, the swap with its way
//!   back, the hand-over to the new build and its confirmation.
//! * [`updater`] and [`state`]: the state machine the application watches,
//!   and what is kept between runs.

pub mod checksums;
pub mod download;
pub mod http;
pub mod install;
pub mod launch;
pub mod package;
pub mod release;
pub mod state;
pub mod tools;
pub mod trust;
pub mod updater;
pub mod version;

pub use http::{CurlHttp, Http};
pub use install::{Install, Target, Why};
pub use release::Platform;
pub use state::{Layout, Offer};
pub use tools::{SystemTools, Tools};
pub use trust::Trust;
pub use updater::{Config, Snapshot, Stage, State, Updater};
pub use version::Version;

/// The environment variable that switches the updater off for a run.
pub const DISABLE_ENV: &str = "LEON_NO_UPDATE";

/// The first check, after the window is up.
pub const FIRST_CHECK_AFTER: std::time::Duration = std::time::Duration::from_secs(8);

/// How often a check is made while Leon runs.
pub const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// The wait before the next check, spread by up to a sixth either way so that
/// the computers of a team do not all ask at the same second. `entropy` is any
/// number that varies between runs.
pub fn jittered_interval(entropy: u64) -> std::time::Duration {
    let base = CHECK_EVERY.as_secs();
    let spread = base / 6;
    let offset = entropy % (2 * spread + 1);
    std::time::Duration::from_secs(base - spread + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_is_six_hours_give_or_take_a_sixth() {
        let six = CHECK_EVERY.as_secs();
        for entropy in [0, 1, 12345, u64::MAX, 3_600] {
            let wait = jittered_interval(entropy).as_secs();
            assert!(wait >= six - six / 6 && wait <= six + six / 6, "{wait}");
        }
        assert_ne!(jittered_interval(0), jittered_interval(1000));
    }
}
