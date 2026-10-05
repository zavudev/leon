//! The log: where it is written and how much, changeable while Leon runs.
//!
//! Leon logs to standard error. [`init`] installs the subscriber with a
//! filter that can be replaced afterwards ([`set_level`]), which is how the
//! `log_level` setting works without a restart. The `RUST_LOG` variable, when
//! set, is the user's own filter and wins: the setting then does nothing.

use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// The handle that replaces the filter of the running subscriber.
pub type Handle = reload::Handle<EnvFilter, Registry>;

static HANDLE: OnceLock<Handle> = OnceLock::new();

/// The filter a level of the setting means: that level for Leon's own crates
/// and warnings for everything else.
pub fn filter_for(level: &str) -> String {
    let own = |level: &str| {
        [
            "leon",
            "leon_core",
            "leon_remote",
            "leon_history",
            "leon_term",
        ]
        .iter()
        .map(|name| format!("{name}={level}"))
        .collect::<Vec<_>>()
        .join(",")
    };
    match level {
        "error" => "error".to_owned(),
        "warn" => "warn".to_owned(),
        "debug" => format!("warn,{}", own("debug")),
        "trace" => format!("warn,{}", own("trace")),
        _ => format!("warn,{}", own("info")),
    }
}

/// Starts logging to standard error at the default level, or at the user's
/// own `RUST_LOG`.
pub fn init() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter_for("info")));
    let (layer, handle) = reload::Layer::new(filter);
    tracing_subscriber::registry()
        .with(layer)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
    let _ = HANDLE.set(handle);
}

/// Changes the level of the running log to one of the setting. Does nothing
/// when no log was started here or when `RUST_LOG` is set.
pub fn set_level(level: &str) {
    if let Some(handle) = HANDLE.get() {
        apply(handle, level, std::env::var_os("RUST_LOG").is_some());
    }
}

/// Puts the filter of `level` on `handle`, unless `env_wins`.
pub fn apply(handle: &Handle, level: &str, env_wins: bool) -> bool {
    if env_wins {
        return false;
    }
    handle.reload(EnvFilter::new(filter_for(level))).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Handle {
        let (layer, handle) = reload::Layer::new(EnvFilter::new(filter_for("info")));
        // Kept alive by the test through the handle's weak reference.
        let subscriber = Box::leak(Box::new(Registry::default().with(layer)));
        let _ = subscriber;
        handle
    }

    #[test]
    fn each_level_names_leons_own_crates_and_quiets_the_rest() {
        assert_eq!(filter_for("error"), "error");
        assert_eq!(filter_for("warn"), "warn");
        assert!(filter_for("info").starts_with("warn,leon=info,leon_core=info"));
        assert!(filter_for("debug").contains("leon_term=debug"));
        assert!(filter_for("trace").contains("leon_remote=trace"));
        assert_eq!(filter_for("nonsense"), filter_for("info"));
    }

    #[test]
    fn the_level_of_a_running_log_is_replaced_and_rust_log_wins() {
        let handle = registry();
        let now = |handle: &Handle| handle.with_current(|filter| filter.to_string()).unwrap();
        assert!(now(&handle).contains("leon=info"));
        assert!(apply(&handle, "debug", false));
        assert!(now(&handle).contains("leon=debug"));
        assert!(
            !apply(&handle, "trace", true),
            "the user's own filter stays"
        );
        assert!(now(&handle).contains("leon=debug"));
    }
}
