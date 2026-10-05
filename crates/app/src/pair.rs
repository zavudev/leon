//! Connecting a machine with a code: what is said and what is done, without a
//! window.
//!
//! A person types the short code another Leon shows under "Share this
//! machine". Both computers dial out to the relay, so there is nothing to open
//! or set up on either network; the code (through a password-authenticated key
//! exchange) makes sure the two computers have found each other and nobody
//! between them, the relay included, can read or alter what follows.
//!
//! The checklist has five steps: reach the relay, find the computer, verify the
//! code, open the secure channel, and (once the machine is added) look for the
//! agents installed there. [`pair`] runs the first four and reports each; the
//! screen adds the fifth from the engine's probe. A failure names the step and
//! says, in plain words, what to do: the relay being down is never blamed on
//! the person's own computer.

use std::sync::Arc;
use std::time::Duration;

use leon_link::relay_client::{dial, DialError, Target};
use leon_link::{pair_as_client, CodeShape, Identity, PairError, PairedHost, PairingCode};
use leon_remote::relay::{describe, key_hex};
use leon_remote::RelayHub;
use leon_wire::relay::RelayErrorCode;

/// The two sentences at the top of "With a code".
pub const WHAT_IT_IS: &str = "On the other computer, open Leon and choose Share this machine: it shows a short code. Type it here and the two computers connect through a relay, with nothing to open or set up on either network.";
/// The sentence about privacy.
pub const HOW_IT_IS_SAFE: &str = "The connection is end-to-end encrypted with keys only the two computers hold, so the relay only passes along bytes it cannot read.";

/// One line of the checklist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// Reach the relay.
    Relay,
    /// Find the computer showing the code.
    Find,
    /// Verify the code.
    Verify,
    /// Open the secure channel.
    Channel,
    /// Agents installed there.
    Agents,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Step; 5] = [
        Step::Relay,
        Step::Find,
        Step::Verify,
        Step::Channel,
        Step::Agents,
    ];

    /// What the line says.
    pub fn label(self) -> &'static str {
        match self {
            Step::Relay => "Reach the relay",
            Step::Find => "Find the other computer",
            Step::Verify => "Verify the code",
            Step::Channel => "Open the secure channel",
            Step::Agents => "Look for agents installed there",
        }
    }

    /// The position in [`Step::ALL`].
    pub fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// How a step is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// Not reached yet.
    Pending,
    /// Under way.
    Running,
    /// Done.
    Passed,
    /// Failed.
    Failed,
}

/// Why connecting failed, in words for a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The step it stopped at.
    pub step: Step,
    /// What happened, in a sentence.
    pub headline: String,
    /// What to do about it.
    pub advice: String,
}

/// What a typed code looks like so far, as a line to show.
pub fn code_hint(typed: &str) -> (String, bool) {
    match leon_link::code::shape(typed) {
        CodeShape::Empty => ("Type the code shown on the other computer.".into(), false),
        CodeShape::Incomplete { have, need } => {
            (format!("{} more character(s).", need - have), false)
        }
        CodeShape::Complete => ("The code looks right.".into(), true),
        CodeShape::BadCharacter(c) => (
            format!("“{c}” is not used in codes: they never contain 0, 1, I or O."),
            false,
        ),
        CodeShape::TooLong => ("That is longer than a code.".into(), false),
    }
}

fn relay_unreachable(url: &str, detail: &str) -> Failure {
    Failure {
        step: Step::Relay,
        headline: format!("Cannot reach the relay at {url} ({detail})."),
        advice: "The relay service is operated by Zavu and may not be live yet. This is about the relay, not about your computer. If you run your own relay, check the address in Settings under Machines, Relay server.".into(),
    }
}

fn from_dial(url: &str, error: &DialError) -> Failure {
    match error {
        DialError::Unreachable(detail) => relay_unreachable(url, detail),
        DialError::Refused(e) if e.code == RelayErrorCode::HostNotFound => Failure {
            step: Step::Find,
            headline: "No computer is showing that code.".into(),
            advice: "Check the code for typing mistakes. A code lasts ten minutes and works once: on the other computer choose New code. Make sure Share this machine is on there and that it is online.".into(),
        },
        DialError::Refused(e) if e.code == RelayErrorCode::RateLimited => Failure {
            step: Step::Find,
            headline: "Too many attempts for that computer just now.".into(),
            advice: "Wait a minute and try again.".into(),
        },
        DialError::Refused(e) => Failure {
            step: Step::Relay,
            headline: format!("The relay at {url} refused the connection: {}", e.message),
            advice: "Try again in a moment.".into(),
        },
        DialError::Protocol => Failure {
            step: Step::Relay,
            headline: format!("The server at {url} does not speak the relay protocol."),
            advice: "Check the address in Settings under Machines, Relay server.".into(),
        },
    }
}

fn from_pair(error: &PairError) -> Failure {
    let (headline, advice) = match error {
        PairError::Expired => (
            "That code has expired.",
            "Codes last ten minutes. On the other computer choose New code.",
        ),
        PairError::Burned => (
            "That code was burned by too many wrong attempts.",
            "On the other computer choose New code and type it carefully.",
        ),
        PairError::AlreadyUsed => (
            "That code was already used.",
            "A code works once. On the other computer choose New code.",
        ),
        PairError::NotAccepted => (
            "The other computer did not accept the code.",
            "It is wrong, expired or already used. Check it against the screen of the other computer; it allows five wrong attempts.",
        ),
        PairError::Denied => (
            "Someone on the other computer declined.",
            "Ask them to allow the connection when it is offered there, then try again.",
        ),
        PairError::Timeout | PairError::Closed => (
            "The other computer stopped answering.",
            "It may have gone to sleep or lost its connection. Try again.",
        ),
        PairError::Protocol | PairError::Link(_) => (
            "The two computers could not agree on a secure channel.",
            "Make sure both run a recent Leon, then try again.",
        ),
    };
    Failure {
        step: Step::Verify,
        headline: headline.into(),
        advice: advice.into(),
    }
}

/// Pairs with the host showing `code` through the relay at `relay_url`, then
/// opens the durable connection to it. `report` is told how each step goes.
pub async fn pair(
    relay_url: &str,
    hub: &Arc<RelayHub>,
    code: &PairingCode,
    device_name: &str,
    mut report: impl FnMut(Step, StepState),
) -> Result<PairedHost, Failure> {
    report(Step::Relay, StepState::Running);
    let pipe = match dial(relay_url, &Target::Room(code.room()), None).await {
        Ok(pipe) => pipe,
        Err(error) => {
            let failure = from_dial(relay_url, &error);
            if failure.step == Step::Find {
                report(Step::Relay, StepState::Passed);
            }
            report(failure.step, StepState::Failed);
            return Err(failure);
        }
    };
    report(Step::Relay, StepState::Passed);
    report(Step::Find, StepState::Passed);
    report(Step::Verify, StepState::Running);
    let identity: &Arc<Identity> = hub.identity();
    let paired = match pair_as_client(pipe, identity, code, device_name).await {
        Ok(paired) => paired,
        Err(error) => {
            let failure = from_pair(&error);
            report(Step::Verify, StepState::Failed);
            return Err(failure);
        }
    };
    report(Step::Verify, StepState::Passed);
    report(Step::Channel, StepState::Running);
    let channel = async {
        let client = hub
            .ensure(
                &paired.host_id.to_string(),
                &key_hex(&paired.host_key),
                relay_url,
            )
            .map_err(|e| e.to_string())?;
        client
            .wait_online(Duration::from_secs(20))
            .await
            .map_err(|_| {
                describe(&client.state(), relay_url).unwrap_or_else(|| "not online".into())
            })?;
        Ok::<_, String>(())
    };
    match channel.await {
        Ok(()) => {
            report(Step::Channel, StepState::Passed);
            Ok(paired)
        }
        Err(why) => {
            report(Step::Channel, StepState::Failed);
            Err(Failure {
                step: Step::Channel,
                headline: format!("The secure channel did not open: {why}"),
                advice: "The pairing worked, so the machine is saved; it will connect when the other computer is reachable.".into(),
            })
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use leon_host::{Host, HostConfig, RelayState};
    use leon_link::test_support::TestRelay;
    use std::sync::Mutex;

    #[test]
    fn the_hint_follows_the_typing() {
        assert!(code_hint("").0.contains("Type the code"));
        assert_eq!(code_hint("AB2").0, "7 more character(s).");
        assert!(code_hint("ABCD-EFG-HJK").1);
        assert!(code_hint("AB0").0.contains("never contain 0, 1, I or O"));
        assert!(code_hint("ABCDEFGHJKM").0.contains("longer"));
    }

    #[test]
    fn the_explanation_is_two_sentences_about_the_relay_and_privacy() {
        assert!(WHAT_IT_IS.contains("relay") && WHAT_IT_IS.contains("short code"));
        assert!(HOW_IT_IS_SAFE.contains("end-to-end encrypted"));
    }

    fn hub() -> Arc<RelayHub> {
        RelayHub::new(
            Arc::new(Identity::generate()),
            "me",
            tokio::runtime::Handle::current(),
        )
    }

    async fn host_at(url: &str) -> Host {
        let host = Host::start(
            HostConfig::new(url, "box"),
            Arc::new(Identity::generate()),
            None,
        )
        .unwrap();
        let mut state = host.watch_relay();
        while *state.borrow_and_update() != RelayState::Online {
            state.changed().await.unwrap();
        }
        host
    }

    #[allow(clippy::type_complexity)]
    fn log() -> (
        Arc<Mutex<Vec<(Step, StepState)>>>,
        impl FnMut(Step, StepState),
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (seen, move |s, st| sink.lock().unwrap().push((s, st)))
    }

    #[tokio::test]
    async fn a_right_code_passes_every_step_in_order() {
        let relay = TestRelay::start().await;
        let host = host_at(&relay.url()).await;
        let info = host.new_pairing_code().await;
        let (seen, report) = log();
        let paired = pair(
            &relay.url(),
            &hub(),
            &PairingCode::parse(&info.code).unwrap(),
            "me",
            report,
        )
        .await
        .unwrap();
        assert_eq!(paired.host_name, "box");
        let seen = seen.lock().unwrap().clone();
        assert!(seen.contains(&(Step::Relay, StepState::Passed)));
        assert!(seen.contains(&(Step::Verify, StepState::Passed)));
        assert_eq!(seen.last(), Some(&(Step::Channel, StepState::Passed)));
    }

    #[tokio::test]
    async fn a_wrong_code_fails_at_verify_with_advice_that_is_about_the_code() {
        let relay = TestRelay::start().await;
        let host = host_at(&relay.url()).await;
        let info = host.new_pairing_code().await;
        let wrong = PairingCode::parse(&format!("{}{}", info.room, "222222")).unwrap();
        let (_, report) = log();
        let failure = pair(&relay.url(), &hub(), &wrong, "me", report)
            .await
            .unwrap_err();
        assert_eq!(failure.step, Step::Verify);
        assert!(failure.advice.contains("five wrong attempts"));
    }

    #[tokio::test]
    async fn a_code_nobody_shows_says_so_and_not_that_the_relay_is_down() {
        let relay = TestRelay::start().await;
        let (seen, report) = log();
        let code = PairingCode::parse("ABCD-EFG-HJK").unwrap();
        let failure = pair(&relay.url(), &hub(), &code, "me", report)
            .await
            .unwrap_err();
        assert_eq!(failure.step, Step::Find);
        assert!(failure
            .headline
            .contains("No computer is showing that code"));
        assert!(seen
            .lock()
            .unwrap()
            .contains(&(Step::Relay, StepState::Passed)));
    }

    #[tokio::test]
    async fn an_unreachable_relay_names_the_configured_address_and_does_not_blame_the_user() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("ws://127.0.0.1:{port}");
        let (_, report) = log();
        let code = PairingCode::parse("ABCD-EFG-HJK").unwrap();
        let failure = pair(&url, &hub(), &code, "me", report).await.unwrap_err();
        assert_eq!(failure.step, Step::Relay);
        assert!(failure.headline.contains(&url), "{}", failure.headline);
        assert!(failure.advice.contains("not about your computer"));
        assert!(failure.advice.contains("not be live yet"));
    }

    #[tokio::test]
    async fn a_used_code_is_reported_as_used() {
        let relay = TestRelay::start().await;
        let host = host_at(&relay.url()).await;
        let info = host.new_pairing_code().await;
        let code = PairingCode::parse(&info.code).unwrap();
        let (_, report) = log();
        pair(&relay.url(), &hub(), &code, "me", report)
            .await
            .unwrap();
        // The room is closed once the code is used, so a second try finds nobody.
        let (_, report) = log();
        let failure = pair(&relay.url(), &hub(), &code, "me", report)
            .await
            .unwrap_err();
        assert!(matches!(failure.step, Step::Find | Step::Verify));
    }
}
