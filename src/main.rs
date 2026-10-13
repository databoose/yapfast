// Matrix sync's encrypted-event futures exceed rustc's default trait depth.
#![recursion_limit = "256"]

#[cfg(feature = "desktop")]
mod custom_emoji;
#[cfg(feature = "desktop")]
mod emoji;
#[cfg(feature = "desktop")]
mod emoji_font;
#[cfg(feature = "desktop")]
mod graphics;
#[cfg(feature = "desktop")]
mod notify;
#[cfg(feature = "desktop")]
mod paste;
#[cfg(feature = "desktop")]
mod ui;
#[cfg(feature = "desktop")]
mod zoom;

use anyhow::{Context, Result, bail};
use talk_core::{cli, logging, runtime::BackgroundRuntime};
#[cfg(feature = "matrix")]
use talk_core::{experiment, voice_demo};

/// glibc gives threads that contend for the heap their own arenas, up to eight
/// per core, and each arena keeps its freed memory. Talk runs dozens of threads.
/// In a ten-person call two arenas held 12 MiB less heap at the same CPU.
fn limit_heap_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        const M_ARENA_MAX: std::ffi::c_int = -8;
        unsafe extern "C" {
            fn mallopt(parameter: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: this runs first in main, before Talk starts any other thread.
        unsafe { mallopt(M_ARENA_MAX, 2) };
    }
}

fn main() -> Result<()> {
    limit_heap_arenas();
    // A stream worker starts nothing else: no UI, runtime or TLS.
    #[cfg(feature = "discord")]
    if std::env::args().nth(1).as_deref() == Some(talk_core::stream_worker::ARGUMENT) {
        logging::init_to_stderr();
        return talk_core::stream_worker::run();
    }
    logging::init();
    talk_core::init_tls()?;

    let runtime = BackgroundRuntime::new().context("could not start background workers")?;
    let mut arguments: Vec<_> = std::env::args().skip(1).collect();
    let renderer = renderer_argument(&mut arguments)?;
    #[cfg(feature = "matrix")]
    if arguments
        .first()
        .is_some_and(|argument| argument == "session")
    {
        return runtime.block_on(cli::session(&arguments[1..]));
    }
    match arguments.as_slice() {
        [] => launch(
            runtime,
            talk_core::default_directory()?,
            false,
            renderer,
            &arguments,
        ),
        [argument] if argument == "--preview" => launch(
            runtime,
            talk_core::default_directory()?,
            true,
            renderer,
            &arguments,
        ),
        [argument, path] if argument == "--data-dir" => {
            launch(runtime, path.into(), false, renderer, &arguments)
        }
        [preview, argument, path] if preview == "--preview" && argument == "--data-dir" => {
            launch(runtime, path.into(), true, renderer, &arguments)
        }
        [argument] if argument == "--smoke-test" => {
            runtime.block_on(runtime.spawn(talk_core::check_build()))?;
            println!(
                "Talk {}: Tokio, LiveKit{}{} build check passed.",
                env!("CARGO_PKG_VERSION"),
                if cfg!(feature = "matrix") {
                    ", Matrix SDK, MatrixRTC"
                } else {
                    ""
                },
                if cfg!(feature = "discord") {
                    ", Discord voice"
                } else {
                    ""
                },
            );
            Ok(())
        }
        #[cfg(feature = "matrix")]
        [argument] if argument == "--voice-demo" => {
            runtime.block_on(voice_demo::provision()).map(|_| ())
        }
        [argument] if argument == "--audio-devices" => cli::audio_devices(),
        #[cfg(feature = "matrix")]
        [argument] if argument == "--call-experiment" => runtime.block_on(experiment::run()),
        #[cfg(feature = "matrix")]
        [argument, resume, path] if argument == "--call-experiment" && resume == "--resume" => {
            runtime.block_on(experiment::resume(path.into()))
        }
        [argument] if argument == "--help" || argument == "-h" => {
            println!(
                "Usage: talk [--preview] [--data-dir PATH] [--renderer gpu|software]\n       talk [--smoke-test | --help]\n       talk --call-experiment [--resume PATH]\n       talk [--voice-demo | --audio-devices]\n       talk session login --homeserver URL --user USER --password-stdin [--data-dir PATH]\n       talk session status [--data-dir PATH]\n       talk session recover --key-stdin [--data-dir PATH]\n       talk session logout [--data-dir PATH]\n\nWithout arguments, opens native Slint chat and restores a saved session.\n--renderer selects Skia Software or Skia GPU for this start, over the setting.\nIf Skia GPU fails to start, Talk restarts with Skia Software.\n--preview opens local sample conversations without connecting to Matrix.\n--smoke-test checks the linked stack without signing in or opening audio devices.\n--call-experiment creates disposable local accounts and checks encrypted synthetic audio. Start scripts/test-stack up first.\n--voice-demo creates two local accounts and a server with encrypted text and voice channels.\n--audio-devices lists native audio devices and cameras without recording.\nSession commands support password sign-in. Secrets are read from stdin."
            );
            Ok(())
        }
        _ => bail!("unknown argument; run talk --help"),
    }
}

/// Set only for a process that replaces a failed GPU start.
#[cfg(feature = "desktop")]
const GPU_FALLBACK_REASON: &str = "TALK_GPU_FALLBACK_REASON";

#[cfg(feature = "desktop")]
fn launch(
    runtime: BackgroundRuntime,
    root: std::path::PathBuf,
    preview: bool,
    renderer: Option<&str>,
    arguments: &[String],
) -> Result<()> {
    let renderer = renderer.map(|renderer| match renderer {
        "gpu" => graphics::Renderer::Gpu,
        _ => graphics::Renderer::Software,
    });
    let fallback = std::env::var(GPU_FALLBACK_REASON).ok();
    #[cfg(feature = "discord")]
    talk_core::keep_discord_files(&root);
    match ui::run(runtime, root, preview, renderer, fallback)? {
        ui::Exit::Closed => Ok(()),
        ui::Exit::UseSoftware(reason) => relaunch(arguments, Some(&reason)),
        ui::Exit::Restart => relaunch(arguments, None),
    }
}

/// The window system allows one renderer per process, so a failed GPU start,
/// or a changed renderer setting, continues in a new process after this one
/// released its session and runtime. `arguments` no longer hold `--renderer`,
/// so a restart follows the setting.
#[cfg(feature = "desktop")]
fn relaunch(arguments: &[String], gpu_failure: Option<&str>) -> Result<()> {
    let mut command = std::process::Command::new(
        std::env::current_exe().context("could not find Talk to restart it")?,
    );
    command.args(arguments);
    match gpu_failure {
        Some(reason) => command
            .args(["--renderer", "software"])
            .env(GPU_FALLBACK_REASON, reason),
        None => command.env_remove(GPU_FALLBACK_REASON),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("could not restart Talk")
    }
    #[cfg(not(unix))]
    {
        let status = command.status().context("could not restart Talk")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(not(feature = "desktop"))]
fn launch(
    _: BackgroundRuntime,
    _: std::path::PathBuf,
    _: bool,
    _: Option<&str>,
    _: &[String],
) -> Result<()> {
    bail!(
        "this build has no desktop UI; run --help for headless commands or rebuild with default features"
    )
}

/// The renderer `--renderer` asks for; without it, the setting decides.
fn renderer_argument(arguments: &mut Vec<String>) -> Result<Option<&'static str>> {
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--renderer")
    else {
        return Ok(None);
    };
    let renderer = match arguments.get(index + 1).map(String::as_str) {
        Some("gpu") => "gpu",
        Some("software") => "software",
        _ => bail!("--renderer requires gpu or software"),
    };
    arguments.drain(index..index + 2);
    if arguments.iter().any(|argument| argument == "--renderer") {
        bail!("specify --renderer once");
    }
    Ok(Some(renderer))
}
