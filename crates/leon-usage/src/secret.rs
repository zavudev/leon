//! A value that must never be printed.
//!
//! The credential an opt-in source reads lives in a [`Secret`] from the moment
//! it is read until the request is sent. `Debug` prints a placeholder, there is
//! no `Display`, no `Serialize` and no `Clone`, so it cannot end up in a log, a
//! message or the store by accident; the one way out is [`Secret::expose`],
//! used where the request is written.

/// A credential.
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_shows_in_debug_output() {
        let secret = Secret::new("sk-ant-oat01-do-not-print");
        let text = format!("{secret:?} {:?}", Some(&secret));
        assert!(!text.contains("sk-ant"));
        assert!(text.contains("***"));
    }
}
