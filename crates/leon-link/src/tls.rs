//! TLS for the relay connection, with one crypto provider chosen on purpose.
//!
//! rustls 0.23 needs a crypto provider. The tree enables exactly one, `ring`
//! (which `snow` already builds on every platform; `aws-lc-rs` would need
//! cmake and NASM on Windows), and this module makes that choice explicit
//! in three ways so that nothing depends on a global that nobody set:
//!
//! * [`client_config`] builds the `ClientConfig` with the provider given
//!   (`builder_with_provider`), never from process-wide state, and the relay
//!   client hands it to the WebSocket library as a ready connector;
//! * [`init`] also installs the provider as the process default, once and
//!   harmlessly when called again, for any other rustls user in the process;
//!   the applications call it first thing in `main`;
//! * the roots are the system's native certificates and nothing else.
//!   Tests may add a root of their own through [`trust_for_tests`], which
//!   exists only in test builds and with the `test-support` feature.

use std::sync::Arc;

#[cfg(any(test, feature = "test-support"))]
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

/// The crypto provider of every TLS connection Leon makes.
pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Installs the provider as the process default. Safe to call any number of
/// times, from any thread; an already installed default is left alone.
pub fn init() {
    // `Err` means a default is already installed, ours or another's: fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(any(test, feature = "test-support"))]
fn extra_roots() -> &'static std::sync::Mutex<Vec<CertificateDer<'static>>> {
    static EXTRA: std::sync::Mutex<Vec<CertificateDer<'static>>> =
        std::sync::Mutex::new(Vec::new());
    &EXTRA
}

/// Makes `root` (a DER certificate) trusted by the relay connections made from
/// now on. For tests that run a TLS relay of their own; production builds
/// have no such seam and trust the system's roots only.
#[cfg(any(test, feature = "test-support"))]
pub fn trust_for_tests(root: CertificateDer<'static>) {
    if let Ok(mut list) = extra_roots().lock() {
        list.push(root);
    }
}

fn native_roots() -> RootCertStore {
    let mut store = RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    let (added, ignored) = store.add_parsable_certificates(loaded.certs);
    tracing::debug!(added, ignored, errors = loaded.errors.len(), "native roots");
    #[cfg(any(test, feature = "test-support"))]
    if let Ok(list) = extra_roots().lock() {
        for root in list.iter() {
            let _ = store.add(root.clone());
        }
    }
    store
}

/// The client configuration of relay connections: the ring provider, the
/// safe protocol defaults, and the roots. It does not touch the process
/// default, so it cannot panic for lack of one.
pub fn client_config() -> Result<Arc<ClientConfig>, String> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_root_certificates(native_roots())
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The connector for the WebSocket library, built once per process unless
/// tests added roots in between (so that is rebuilt then).
pub fn connector() -> Result<tokio_tungstenite::Connector, String> {
    init();
    #[cfg(any(test, feature = "test-support"))]
    {
        client_config().map(tokio_tungstenite::Connector::Rustls)
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        static CONFIG: std::sync::OnceLock<Result<Arc<ClientConfig>, String>> =
            std::sync::OnceLock::new();
        CONFIG
            .get_or_init(client_config)
            .clone()
            .map(tokio_tungstenite::Connector::Rustls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_builds_with_no_prior_init() {
        // A fresh process default is absent here or set by another test: the
        // explicit provider must not care.
        let config = client_config().unwrap();
        assert!(config.alpn_protocols.is_empty());
    }

    #[test]
    fn initialising_twice_is_harmless() {
        init();
        init();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
        assert!(connector().is_ok());
    }

    /// The relay is reached over real TLS (a throwaway certificate for
    /// `localhost`, trusted through the test seam), registered with, dialled
    /// and spoken through: the path a `wss://` relay takes.
    #[tokio::test]
    async fn a_wss_relay_is_reached_registered_with_and_dialled() {
        use crate::identity::Identity;
        use crate::relay_client::{dial, register, Target};
        let relay = crate::test_support::TestRelay::start_tls_self_signed().await;
        let host = Identity::generate();
        let mut link = register(&relay.tls_url(), &host, None).await.unwrap();
        let mut client = dial(&relay.tls_url(), &Target::Host(host.host_id()), None)
            .await
            .unwrap();
        let incoming = link.accept().await.unwrap();
        client.send(b"hello over tls".to_vec()).await.unwrap();
        let mut pipe = incoming.pipe;
        assert_eq!(pipe.recv().await.unwrap(), b"hello over tls".to_vec());
        pipe.send(b"back".to_vec()).await.unwrap();
        assert_eq!(client.recv().await.unwrap(), b"back".to_vec());
        relay.stop().await;
    }

    #[tokio::test]
    async fn a_relay_whose_certificate_is_not_trusted_is_a_clear_error_not_a_hang() {
        use crate::relay_client::{dial, Target};
        // A plain-TCP listener that is not a TLS server at all.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(b"not tls at all\r\n").await;
        });
        let id = crate::identity::Identity::generate().host_id();
        let error = dial(&format!("wss://localhost:{port}"), &Target::Host(id), None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, crate::relay_client::DialError::Unreachable(_)),
            "{error:?}"
        );
    }

    #[test]
    fn a_failure_of_the_secure_setup_says_so_in_words() {
        let error = crate::relay_client::DialError::Secure("the TLS library failed".into());
        assert_eq!(
            error.to_string(),
            "The secure connection could not be set up: the TLS library failed"
        );
    }
}
