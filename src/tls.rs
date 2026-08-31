/// Install the process-wide Rustls provider used by HTTP and WebSocket clients.
///
/// Reqwest's `rustls-no-provider` feature deliberately leaves this choice to
/// the application. Treat an already-installed provider as success so every
/// client construction path can call this safely, including parallel tests.
pub(crate) fn install_crypto_provider() -> Result<(), &'static str> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }

    match rustls::crypto::ring::default_provider().install_default() {
        Ok(()) => Ok(()),
        Err(_) if rustls::crypto::CryptoProvider::get_default().is_some() => Ok(()),
        Err(_) => Err("could not install the Ring TLS crypto provider"),
    }
}
