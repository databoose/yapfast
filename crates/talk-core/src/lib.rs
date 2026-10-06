//! Talk's application state and native network services, independent of the UI.
//! Matrix and Discord are the `matrix` and `discord` Cargo features.
// Matrix sync's encrypted-event futures exceed rustc's default trait depth.
#![recursion_limit = "256"]
// A build without networks only runs the preview and device checks.
#![cfg_attr(not(any(feature = "matrix", feature = "discord")), allow(dead_code))]

use anyhow::Context;

pub mod application;
pub mod backend;
#[cfg(feature = "matrix")]
pub mod calls;
pub mod chat;
pub mod cli;
pub mod discord;
#[cfg(feature = "discord")]
pub mod discord_audio;
#[cfg(feature = "discord")]
pub mod discord_text;
#[cfg(feature = "discord")]
mod discord_voice;
#[cfg(feature = "matrix")]
pub mod experiment;
pub mod links;
pub mod logging;
#[cfg(feature = "matrix")]
mod matrix_chat;
#[cfg(feature = "matrix")]
mod matrix_voice;
pub mod meter;
pub mod runtime;
#[cfg(feature = "matrix")]
pub mod session;
pub mod voice;
#[cfg(feature = "matrix")]
pub mod voice_demo;

/// Install the TLS provider before starting Matrix or calling services.
pub fn init_tls() -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("could not initialize the TLS provider"))
}

/// Where Talk keeps its sessions and local settings unless given --data-dir.
pub fn default_directory() -> anyhow::Result<std::path::PathBuf> {
    directories::ProjectDirs::from("chat", "Talk", "Talk")
        .map(|dirs| dirs.data_local_dir().to_owned())
        .context("could not find an application data directory; pass --data-dir")
}

/// Check linked native services without a session, network request or audio device.
pub async fn check_build() {
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    #[cfg(feature = "matrix")]
    {
        calls::check_native_build();
        let _ = matrix_sdk::Client::builder();
    }
    // Links songbird's libopus beside libwebrtc's; both must resolve.
    #[cfg(feature = "discord")]
    let _ = talk_discord::voice::live_microphone();
}

/// Cargo runs library tests from the crate directory; retain workspace-local fixtures.
#[cfg(all(test, feature = "matrix"))]
fn test_data_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".local")
}
