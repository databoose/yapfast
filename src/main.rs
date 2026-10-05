// Matrix sync's encrypted-event futures exceed rustc's default trait depth.
#![recursion_limit = "256"]

#[cfg(feature = "desktop")]
mod graphics;
#[cfg(feature = "desktop")]
mod ui;

use anyhow::{Context, Result, bail};
use talk_core::{cli, experiment, logging, runtime::BackgroundRuntime, session, voice_demo};

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
    logging::init();
    talk_core::init_tls()?;

    let runtime = BackgroundRuntime::new().context("could not start background workers")?;
    let mut arguments: Vec<_> = std::env::args().skip(1).collect();
    let renderer = renderer_argument(&mut arguments)?;
    if arguments
        .first()
        .is_some_and(|argument| argument == "session")
    {
        return runtime.block_on(cli::session(&arguments[1..]));
    }
    match arguments.as_slice() {
        [] => launch(
            runtime,
            session::default_directory()?,
            false,
            renderer,
            &arguments,
        ),
        [argument] if argument == "--preview" => launch(
            runtime,
            session::default_directory()?,
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
                "Usage: talk [--preview] [--data-dir PATH] [--renderer gpu|software]\n       talk [--smoke-test | --help]\n       talk --call-experiment [--resume PATH]\n       talk [--voice-demo | --audio-devices]\n       talk session login --homeserver URL --user USER --password-stdin [--data-dir PATH]\n       talk session status [--data-dir PATH]\n       talk session recover --key-stdin [--data-dir PATH]\n       talk session logout [--data-dir PATH]\n\nWithout arguments, opens native Slint chat and restores a saved session.\n--renderer gpu or software selects one renderer. By default Talk uses Skia GPU\nand switches to Skia Software after a failed GPU start.\n--preview opens local sample conversations without connecting to Matrix.\n--smoke-test checks the linked stack without signing in or opening audio devices.\n--call-experiment creates disposable local accounts and checks encrypted synthetic audio. Start scripts/test-stack up first.\n--voice-demo creates two local accounts and a server with encrypted text and voice channels.\n--audio-devices lists native audio devices without recording.\nSession commands support password sign-in. Secrets are read from stdin."
            );
            Ok(())
        }
        _ => bail!("unknown argument; run talk --help"),
    }
}

/// Set only for a process that replaces a failed automatic GPU start.
#[cfg(feature = "desktop")]
const GPU_FALLBACK_REASON: &str = "TALK_GPU_FALLBACK_REASON";

#[cfg(feature = "desktop")]
fn launch(
    runtime: BackgroundRuntime,
    root: std::path::PathBuf,
    preview: bool,
    renderer: &str,
    arguments: &[String],
) -> Result<()> {
    let mode = match renderer {
        "gpu" => graphics::Mode::Gpu,
        "software" => graphics::Mode::Software,
        _ => graphics::Mode::Auto,
    };
    let reason = std::env::var(GPU_FALLBACK_REASON).ok();
    let plan = graphics::plan(&root, mode, reason);
    match ui::run(runtime, root, preview, plan)? {
        ui::Exit::Closed => Ok(()),
        ui::Exit::UseSoftware(reason) => relaunch_with_software(arguments, &reason),
    }
}

/// The window system allows one renderer per process, so a failed GPU start
/// continues in a new process after this one released its session and runtime.
#[cfg(feature = "desktop")]
fn relaunch_with_software(arguments: &[String], reason: &str) -> Result<()> {
    let mut command = std::process::Command::new(
        std::env::current_exe().context("could not find Talk to restart it")?,
    );
    command
        .args(arguments)
        .args(["--renderer", "software"])
        .env(GPU_FALLBACK_REASON, reason);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("could not restart Talk with software rendering")
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .context("could not restart Talk with software rendering")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(not(feature = "desktop"))]
fn launch(
    _: BackgroundRuntime,
    _: std::path::PathBuf,
    _: bool,
    _: &str,
    _: &[String],
) -> Result<()> {
    bail!(
        "this build has no desktop UI; run --help for headless commands or rebuild with default features"
    )
}

fn renderer_argument(arguments: &mut Vec<String>) -> Result<&'static str> {
    let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--renderer")
    else {
        return Ok("auto");
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
    Ok(renderer)
}
