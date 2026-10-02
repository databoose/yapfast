//! Talk's application state and native Matrix services, independent of the UI.
// Matrix sync's encrypted-event futures exceed rustc's default trait depth.
#![recursion_limit = "256"]

pub mod application;
pub mod backend;
pub mod calls;
pub mod chat;
pub mod cli;
pub mod experiment;
pub mod logging;
pub mod runtime;
pub mod session;
pub mod voice;
pub mod voice_demo;

/// Install the TLS provider before starting Matrix or calling services.
pub fn init_tls() -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("could not initialize the TLS provider"))
}

/// Check linked native services without a session, network request or audio device.
pub async fn check_build() {
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    calls::check_native_build();
    let _ = matrix_sdk::Client::builder();
}

/// Cargo runs library tests from the crate directory; retain workspace-local fixtures.
#[cfg(test)]
fn test_data_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".local")
}
