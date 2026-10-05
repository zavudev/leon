//! Typed identifiers for the entities Leon stores.
//!
//! Every entity is addressed by an opaque string id wrapped in its own
//! newtype, so a project id can never be passed where a machine id is
//! expected. New ids are UUID v7 values: unique without coordination and
//! roughly ordered by creation time, which keeps primary-key indexes compact.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a fresh, unique identifier.
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }

            /// Wraps an identifier that already exists, for example one read
            /// back from the store.
            pub fn from_string(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identifier as text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

typed_id!(
    /// Identifies a [`Machine`](crate::Machine).
    MachineId
);
typed_id!(
    /// Identifies a [`Project`](crate::Project).
    ProjectId
);
typed_id!(
    /// Identifies a [`Worktree`](crate::Worktree).
    WorktreeId
);
typed_id!(
    /// Identifies a [`Session`](crate::Session) inside Leon. This is not the
    /// agent's own session id; see `Session::external_id` for that.
    SessionId
);

impl MachineId {
    /// The stable id of the machine Leon itself runs on. That machine always
    /// exists in the store.
    pub fn local() -> Self {
        Self("local".to_owned())
    }

    /// Whether this id names the built-in local machine.
    pub fn is_local(&self) -> bool {
        self.0 == "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique() {
        assert_ne!(ProjectId::generate(), ProjectId::generate());
    }

    #[test]
    fn the_local_machine_id_is_stable() {
        assert_eq!(MachineId::local(), MachineId::local());
        assert!(MachineId::local().is_local());
        assert!(!MachineId::generate().is_local());
    }

    #[test]
    fn an_id_serialises_as_a_plain_string() {
        let id = SessionId::from_string("abc");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"abc\"");
        assert_eq!(id.to_string(), "abc");
    }
}
