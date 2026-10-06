//! What a new build has to be, given what the running one is.
//!
//! The checksum says the file is the one GitHub holds. The signature says who
//! made it, and is held to the installed build:
//!
//! * a running build that is signed (macOS: by a Team ID; Windows: with an
//!   Authenticode certificate) is replaced only by a build with a whole
//!   signature by the same Team ID or certificate subject. An unsigned build,
//!   a broken one, or one signed by somebody else is refused;
//! * a running build that is not signed (every build until releases are
//!   signed) accepts a build that is not signed either, and says so: the person
//!   is told the update is not signed, in the window and in the log.
//!
//! Linux has no signature scheme to check; the checksum is what there is.

use crate::release::Os;
use crate::tools::Signature;

/// What was found of a build that is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Signed, and whole; by this signer.
    Signed(String),
    /// Not signed by anybody, and the running build is not either.
    Unsigned,
    /// There is no signature scheme on this system.
    NoScheme,
}

/// Whether `staged` may replace `running`.
pub fn judge(os: Os, running: &Signature, staged: &Signature) -> Result<Trust, String> {
    if os == Os::Linux {
        return Ok(Trust::NoScheme);
    }
    match (&running.identity, &staged.identity) {
        (Some(running_id), Some(staged_id)) if staged.valid && running_id == staged_id => {
            Ok(Trust::Signed(staged_id.clone()))
        }
        (Some(running_id), Some(staged_id)) if staged.valid => Err(format!(
            "the new build is signed by {staged_id}, not by {running_id}, which signed this one"
        )),
        (Some(_), Some(_)) => Err("the signature of the new build is not whole".to_owned()),
        (Some(running_id), None) => Err(format!(
            "this build is signed by {running_id} and the new one is not signed"
        )),
        (None, Some(staged_id)) if staged.valid => Ok(Trust::Signed(staged_id.clone())),
        (None, Some(_)) => Err("the signature of the new build is not whole".to_owned()),
        (None, None) => Ok(Trust::Unsigned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad(identity: &str) -> Signature {
        Signature {
            identity: Some(identity.into()),
            valid: false,
        }
    }

    #[test]
    fn a_signed_install_takes_only_the_same_signer() {
        for os in [Os::Macos, Os::Windows] {
            let running = Signature::good("TEAM1");
            assert_eq!(
                judge(os, &running, &Signature::good("TEAM1")),
                Ok(Trust::Signed("TEAM1".into()))
            );
            assert!(judge(os, &running, &Signature::good("TEAM2")).is_err());
            assert!(judge(os, &running, &Signature::none()).is_err());
            assert!(judge(os, &running, &bad("TEAM1")).is_err());
        }
    }

    #[test]
    fn an_unsigned_install_takes_an_unsigned_build_and_says_so() {
        for os in [Os::Macos, Os::Windows] {
            assert_eq!(
                judge(os, &Signature::none(), &Signature::none()),
                Ok(Trust::Unsigned)
            );
        }
    }

    #[test]
    fn an_unsigned_install_may_move_to_a_signed_build_but_not_a_broken_one() {
        assert_eq!(
            judge(Os::Macos, &Signature::none(), &Signature::good("T")),
            Ok(Trust::Signed("T".into()))
        );
        assert!(judge(Os::Macos, &Signature::none(), &bad("T")).is_err());
    }

    #[test]
    fn linux_has_no_signature_to_check() {
        assert_eq!(
            judge(Os::Linux, &Signature::none(), &Signature::none()),
            Ok(Trust::NoScheme)
        );
    }
}
