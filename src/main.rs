// Matrix sync's encrypted-event futures exceed rustc's default trait depth.
#![recursion_limit = "256"]

#[cfg(feature = "desktop")]
mod ui;

use anyhow::{Context, Result, bail};
use talk_core::{cli, experiment, logging, runtime::BackgroundRuntime, session, voice_demo};

fn main() -> Result<()> {
    logging::init();
    talk_core::init_tls()?;

    let runtime = BackgroundRuntime::new().context("could not start background workers")?;
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|argument| argument == "session")
    {
        return runtime.block_on(cli::session(&arguments[1..]));
    }
    match arguments.as_slice() {
        [] => launch(runtime, session::default_directory()?, false),
        [argument] if argument == "--preview" => {
            launch(runtime, session::default_directory()?, true)
        }
        [argument, path] if argument == "--data-dir" => launch(runtime, path.into(), false),
        [preview, argument, path] if preview == "--preview" && argument == "--data-dir" => {
            launch(runtime, path.into(), true)
        }
        [argument] if argument == "--smoke-test" => {
            runtime.block_on(runtime.spawn(talk_core::check_build()))?;
            println!(
                "Talk {}: Tokio, Matrix SDK, MatrixRTC and LiveKit build check passed.",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        [argument] if argument == "--voice-demo" => {
            runtime.block_on(voice_demo::provision()).map(|_| ())
        }
        [argument] if argument == "--audio-devices" => cli::audio_devices(),
        [argument] if argument == "--call-experiment" => runtime.block_on(experiment::run()),
        [argument, resume, path] if argument == "--call-experiment" && resume == "--resume" => {
            runtime.block_on(experiment::resume(path.into()))
        }
        [argument] if argument == "--help" || argument == "-h" => {
            println!(
                "Usage: talk [--preview] [--data-dir PATH]\n       talk [--smoke-test | --help]\n       talk --call-experiment [--resume PATH]\n       talk [--voice-demo | --audio-devices]\n       talk session login --homeserver URL --user USER --password-stdin [--data-dir PATH]\n       talk session status [--data-dir PATH]\n       talk session recover --key-stdin [--data-dir PATH]\n       talk session logout [--data-dir PATH]\n\nWithout arguments, opens native chat and restores a saved session.\n--preview opens local sample conversations without connecting to Matrix.\n--smoke-test checks the linked stack without signing in or opening audio devices.\n--call-experiment creates disposable local accounts and checks encrypted synthetic audio. Start scripts/test-stack up first.\n--voice-demo creates two local accounts and a server with encrypted text and voice channels.\n--audio-devices lists native audio devices without recording.\nSession commands support password sign-in. Secrets are read from stdin."
            );
            Ok(())
        }
        _ => bail!("unknown argument; run talk --help"),
    }
}

#[cfg(feature = "desktop")]
fn launch(runtime: BackgroundRuntime, root: std::path::PathBuf, preview: bool) -> Result<()> {
    ui::run(runtime, root, preview)
}

#[cfg(not(feature = "desktop"))]
fn launch(_: BackgroundRuntime, _: std::path::PathBuf, _: bool) -> Result<()> {
    bail!(
        "this build has no desktop UI; run --help for headless commands or rebuild with default features"
    )
}
