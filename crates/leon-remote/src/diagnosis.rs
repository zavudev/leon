//! Turning what `ssh` printed into an explanation and a fix.
//!
//! OpenSSH reports every problem as a line on standard error and the status
//! 255. [`classify`] reads that pair and answers with a [`Diagnosis`]: what
//! went wrong in plain words, why it matters and what to do. It is pure, so a
//! table of real messages tests it. A message it does not know becomes
//! [`DiagnosisKind::Unknown`], which carries the raw text so nothing is hidden.

/// Which step of reaching a machine a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The computer must be found on the network and answer on its SSH port.
    Reach,
    /// Its host key must be known and unchanged.
    HostKey,
    /// The key must be accepted.
    Login,
    /// The remote shell must be a POSIX one.
    Shell,
}

/// What kind of problem was recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosisKind {
    /// The name does not resolve to an address.
    DnsFailure,
    /// The computer answered that nothing listens on the port.
    ConnectionRefused,
    /// Nothing answered in time.
    TimedOut,
    /// The network has no route to the address.
    NoRoute,
    /// The host key is not in `known_hosts` yet.
    HostKeyUnknown,
    /// The host key differs from the one on record.
    HostKeyChanged,
    /// The server accepts keys and refused ours.
    PublicKeyRefused,
    /// The server only offers a password.
    PasswordOnly,
    /// The server only offers an interactive question.
    InteractiveOnly,
    /// Too many keys were offered.
    TooManyAttempts,
    /// The server hung up during login.
    ConnectionClosed,
    /// The private key file is readable by others.
    KeyPermissions,
    /// The identity file does not exist.
    IdentityMissing,
    /// The key has a passphrase and is not in the agent.
    PassphraseNeeded,
    /// The other computer runs Windows.
    WindowsRemote,
    /// The `ssh` program could not be started.
    SshMissing,
    /// Not recognised: the raw text is the explanation.
    Unknown,
}

impl DiagnosisKind {
    /// The step of the checklist the problem stops at.
    pub fn stage(self) -> Stage {
        use DiagnosisKind as K;
        match self {
            K::DnsFailure | K::ConnectionRefused | K::TimedOut | K::NoRoute | K::SshMissing => {
                Stage::Reach
            }
            K::HostKeyUnknown | K::HostKeyChanged => Stage::HostKey,
            K::WindowsRemote => Stage::Shell,
            _ => Stage::Login,
        }
    }
}

/// What is wrong and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    /// What was recognised.
    pub kind: DiagnosisKind,
    /// A short headline.
    pub title: String,
    /// What happened, in plain words.
    pub explanation: String,
    /// What to do, in plain words.
    pub fix: String,
}

fn make(kind: DiagnosisKind, title: &str, explanation: &str, fix: &str) -> Diagnosis {
    Diagnosis {
        kind,
        title: title.to_owned(),
        explanation: explanation.to_owned(),
        fix: fix.to_owned(),
    }
}

/// Explains the failure of an `ssh` run: its exit status and standard error.
pub fn classify(status: Option<i32>, stderr: &str) -> Diagnosis {
    use DiagnosisKind as K;
    let text = stderr.to_lowercase();
    let has = |needle: &str| text.contains(needle);
    if has("remote host identification has changed") || has("host key for") && has("has changed") {
        return make(
            K::HostKeyChanged,
            "The host key has changed",
            "This computer presented a different identity than the one saved earlier. \
             That happens after a reinstall, but it is also what an impostor looks like.",
            "Leon will not override this. Find out why it changed. If you are sure it is \
             legitimate, remove the old entry yourself with `ssh-keygen -R <host>`.",
        );
    }
    if has("host key verification failed") {
        return make(
            K::HostKeyUnknown,
            "This computer is not trusted yet",
            "Leon has not seen this computer before, so it cannot be sure it is the right one.",
            "Compare the fingerprint with the one on the other computer, then choose \
             \"Trust this computer\".",
        );
    }
    if has("is not recognized as an internal or external command") {
        return make(
            K::WindowsRemote,
            "Windows is not supported yet",
            "The other computer answered with a Windows command prompt.",
            "Use a macOS or Linux computer for now.",
        );
    }
    if has("could not resolve hostname")
        || has("name or service not known")
        || has("nodename nor servname")
        || has("temporary failure in name resolution")
    {
        return make(
            K::DnsFailure,
            "The name does not exist",
            "Your computer could not turn that name into an address.",
            "Check the spelling of the host, or type its IP address instead. \
             Make sure you are on the same network or VPN.",
        );
    }
    if has("connection refused") {
        return make(
            K::ConnectionRefused,
            "The computer refused the connection",
            "It is on, but nothing listens for SSH on that port.",
            "Turn the SSH server on (macOS: System Settings > General > Sharing > Remote Login; \
             Linux: `sudo apt install openssh-server`, then `sudo systemctl enable --now ssh`) \
             and check the port.",
        );
    }
    if has("timed out") || has("did not finish within") {
        return make(
            K::TimedOut,
            "The computer did not answer",
            "The connection started but nothing came back in time.",
            "Check that it is switched on and on the same network or VPN, and that a \
             firewall does not block the SSH port.",
        );
    }
    if has("no route to host") || has("network is unreachable") {
        return make(
            K::NoRoute,
            "There is no route to the computer",
            "Your network does not know how to reach that address.",
            "Check the address and your network or VPN connection.",
        );
    }
    if has("too many authentication failures") {
        return make(
            K::TooManyAttempts,
            "Too many keys were tried",
            "Your SSH agent offered more keys than the server allows attempts.",
            "Name the right key in \"Identity file\" so only that one is offered.",
        );
    }
    if has("unprotected private key file") || has("bad permissions") {
        return make(
            K::KeyPermissions,
            "The key file is too open",
            "SSH refuses a private key that other users can read.",
            "Run `chmod 600` on the key file.",
        );
    }
    if has("no such identity file")
        || (has("identity file") && has("not accessible") && has("no such file"))
    {
        return make(
            K::IdentityMissing,
            "The identity file does not exist",
            "The key file you named is not there.",
            "Pick an existing private key (for example ~/.ssh/id_ed25519) or clear the field.",
        );
    }
    if has("passphrase") {
        return make(
            K::PassphraseNeeded,
            "The key needs its passphrase",
            "The key is protected by a passphrase and Leon cannot type it for you.",
            "Add it to the agent once with `ssh-add <key file>`, then test again.",
        );
    }
    if has("permission denied") {
        let methods = text
            .split_once("permission denied (")
            .and_then(|(_, rest)| rest.split_once(')'))
            .map_or("", |(methods, _)| methods);
        if methods.contains("publickey") || methods.is_empty() {
            return make(
                K::PublicKeyRefused,
                "Your key is not accepted",
                "The computer is there, but it does not know your key.",
                "Authorise your key there with `ssh-copy-id`, then test again.",
            );
        }
        if methods.contains("keyboard-interactive") && !methods.contains("password") {
            return make(
                K::InteractiveOnly,
                "Only an interactive login is offered",
                "The server wants a question answered; Leon needs a key.",
                "Set up key login with `ssh-copy-id`.",
            );
        }
        return make(
            K::PasswordOnly,
            "The server only accepts a password",
            "Leon runs SSH without a terminal, so it cannot ask for a password.",
            "Set up key login with `ssh-copy-id`; afterwards no password is needed.",
        );
    }
    if has("connection closed by")
        || has("connection reset by")
        || has("kex_exchange_identification")
    {
        return make(
            K::ConnectionClosed,
            "The computer hung up",
            "It accepted the connection and then closed it.",
            "Check that the SSH server is healthy and that your address is allowed to log in.",
        );
    }
    if has("cannot start ssh") || has("no such file or directory") && has("ssh:") {
        return make(
            K::SshMissing,
            "The ssh program was not found",
            "Leon uses the ssh of your system and could not start it.",
            "Install OpenSSH and make sure `ssh` is on your PATH.",
        );
    }
    let raw = stderr.trim();
    make(
        K::Unknown,
        "Something went wrong",
        &if raw.is_empty() {
            format!("ssh ended with status {status:?} and said nothing.")
        } else {
            format!("ssh said: {raw}")
        },
        "Run the command shown above in a terminal to see more.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use DiagnosisKind as K;

    #[test]
    fn real_openssh_messages_are_recognised() {
        let table: &[(&str, K)] = &[
            ("ssh: connect to host box port 22: Connection refused", K::ConnectionRefused),
            ("ssh: connect to host box port 22: Operation timed out", K::TimedOut),
            ("ssh: connect to host box port 22: Connection timed out", K::TimedOut),
            ("ssh did not finish within 30s", K::TimedOut),
            ("ssh: connect to host 10.0.0.9 port 22: No route to host", K::NoRoute),
            ("ssh: connect to host 10.0.0.9 port 22: Network is unreachable", K::NoRoute),
            ("ssh: Could not resolve hostname nope: nodename nor servname provided, or not known", K::DnsFailure),
            ("ssh: Could not resolve hostname nope: Name or service not known", K::DnsFailure),
            ("Host key verification failed.", K::HostKeyUnknown),
            ("@@@@\n@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n@@@@\nHost key verification failed.", K::HostKeyChanged),
            ("dev@box: Permission denied (publickey).", K::PublicKeyRefused),
            ("dev@box: Permission denied (publickey,password).", K::PublicKeyRefused),
            ("dev@box: Permission denied (password).", K::PasswordOnly),
            ("dev@box: Permission denied (keyboard-interactive).", K::InteractiveOnly),
            ("Received disconnect from 1.2.3.4 port 22:2: Too many authentication failures", K::TooManyAttempts),
            ("Connection closed by 1.2.3.4 port 22", K::ConnectionClosed),
            ("kex_exchange_identification: read: Connection reset by peer", K::ConnectionClosed),
            ("@         WARNING: UNPROTECTED PRIVATE KEY FILE!          @\nBad permissions: ignore key: /k", K::KeyPermissions),
            ("Warning: Identity file /nope not accessible: No such file or directory.", K::IdentityMissing),
            ("Load key \"/k\": incorrect passphrase supplied to decrypt private key", K::PassphraseNeeded),
            ("Enter passphrase for key '/k':", K::PassphraseNeeded),
            ("'true' is not recognized as an internal or external command,", K::WindowsRemote),
            ("something nobody has seen before", K::Unknown),
        ];
        for (stderr, kind) in table {
            assert_eq!(classify(Some(255), stderr).kind, *kind, "{stderr}");
        }
    }

    #[test]
    fn every_diagnosis_says_what_to_do() {
        for (stderr, _) in [
            ("Connection refused", 0),
            ("Host key verification failed.", 0),
            ("Permission denied (publickey).", 0),
            ("", 0),
        ] {
            let found = classify(Some(255), stderr);
            assert!(!found.title.is_empty() && !found.explanation.is_empty());
            assert!(!found.fix.is_empty());
        }
    }

    #[test]
    fn an_unknown_failure_shows_the_raw_text() {
        let found = classify(Some(255), "weird thing\n");
        assert_eq!(found.kind, K::Unknown);
        assert!(found.explanation.contains("weird thing"));
        assert!(classify(Some(1), "").explanation.contains("status Some(1)"));
    }

    #[test]
    fn a_changed_key_never_suggests_an_override_flag() {
        let found = classify(Some(255), "REMOTE HOST IDENTIFICATION HAS CHANGED");
        assert!(!found.fix.contains("StrictHostKeyChecking"));
        assert!(!found.fix.contains("accept"));
    }

    #[test]
    fn the_stage_follows_the_kind() {
        assert_eq!(K::DnsFailure.stage(), Stage::Reach);
        assert_eq!(K::HostKeyUnknown.stage(), Stage::HostKey);
        assert_eq!(K::PublicKeyRefused.stage(), Stage::Login);
        assert_eq!(K::WindowsRemote.stage(), Stage::Shell);
    }
}
