//! Renderer choice at startup and recovery from a failed GPU start.
//!
//! Automatic mode prefers Skia GPU. A GPU start that fails, or a process that
//! stops before GPU startup is confirmed, is recorded beside the session data;
//! later automatic starts use Skia Software until a newer Talk version or an
//! explicit `--renderer gpu` start succeeds.
use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const RECORD: &str = "graphics.json";

/// The renderer requested on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// GPU first, then Skia Software after a failed GPU start.
    Auto,
    Gpu,
    Software,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Gpu,
    Software,
}

/// The renderer for this process, with the reason for any automatic fallback.
pub struct Plan {
    pub renderer: Renderer,
    pub notice: Option<String>,
    pub start: Option<GpuStart>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Starting,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    version: String,
    state: State,
    reason: String,
}

/// `relaunch_reason` is set when this process replaced a failed GPU process.
pub fn plan(root: &Path, mode: Mode, relaunch_reason: Option<String>) -> Plan {
    match mode {
        Mode::Software => Plan {
            renderer: Renderer::Software,
            notice: relaunch_reason.map(|_| NOTICE.into()),
            start: None,
        },
        Mode::Gpu => Plan {
            renderer: Renderer::Gpu,
            notice: None,
            start: Some(GpuStart::new(root, false)),
        },
        Mode::Auto => {
            let previous = read(root).filter(|record| record.version == VERSION);
            let reason = match previous {
                Some(Record {
                    state: State::Failed,
                    reason,
                    ..
                }) => Some(reason),
                Some(Record {
                    state: State::Starting,
                    ..
                }) => {
                    let reason = "the previous GPU start stopped before it was confirmed";
                    write(root, State::Failed, reason);
                    Some(reason.into())
                }
                None => None,
            };
            if let Some(reason) = reason {
                tracing::warn!(%reason, "using Skia Software after a failed GPU start");
                return Plan {
                    renderer: Renderer::Software,
                    notice: Some(NOTICE.into()),
                    start: None,
                };
            }
            write(root, State::Starting, "");
            Plan {
                renderer: Renderer::Gpu,
                notice: None,
                start: Some(GpuStart::new(root, true)),
            }
        }
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The technical reason stays in the log and the record; the notice is for people.
const NOTICE: &str = "GPU rendering failed to start, so Talk is using software rendering. Start Talk with --renderer gpu to try the GPU again.";

/// Tracks one GPU start until it is confirmed or fails.
pub struct GpuStart {
    root: PathBuf,
    automatic: bool,
    confirmed: Cell<bool>,
}

impl GpuStart {
    fn new(root: &Path, automatic: bool) -> Self {
        Self {
            root: root.into(),
            automatic,
            confirmed: Cell::new(false),
        }
    }

    /// The GPU window has run long enough to trust this renderer. A successful
    /// start also clears an earlier automatic fallback.
    pub fn confirm(&self) {
        if !self.confirmed.replace(true) {
            remove(&self.root);
        }
    }

    /// Returns whether to relaunch with Skia Software. Only an automatic start
    /// relaunches; a failure before confirmation also applies to later starts.
    pub fn failed(&self, reason: &str) -> bool {
        if !self.automatic {
            return false;
        }
        if !self.confirmed.get() {
            write(&self.root, State::Failed, reason);
        }
        true
    }
}

fn read(root: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(root.join(RECORD)).ok()?).ok()
}

/// The record only selects a renderer; failing to store it must not stop Talk.
fn write(root: &Path, state: State, reason: &str) {
    let record = Record {
        version: VERSION.into(),
        state,
        reason: reason.into(),
    };
    let result = std::fs::create_dir_all(root).and_then(|()| {
        std::fs::write(
            root.join(RECORD),
            serde_json::to_vec(&record).expect("graphics record serializes"),
        )
    });
    if let Err(error) = result {
        tracing::warn!(%error, "could not store the graphics startup record");
    }
}

fn remove(root: &Path) {
    match std::fs::remove_file(root.join(RECORD)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, "could not clear the graphics startup record"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfirmed_automatic_start_selects_software_next_time() {
        let directory = tempfile::tempdir().unwrap();
        let first = plan(directory.path(), Mode::Auto, None);
        assert_eq!(first.renderer, Renderer::Gpu);
        assert!(first.notice.is_none());
        // The process stops here, for example in a graphics driver.
        let second = plan(directory.path(), Mode::Auto, None);
        assert_eq!(second.renderer, Renderer::Software);
        assert_eq!(second.notice.as_deref(), Some(NOTICE));
        assert!(
            read(directory.path())
                .unwrap()
                .reason
                .contains("stopped before")
        );
        assert!(second.start.is_none());
        let third = plan(directory.path(), Mode::Auto, None);
        assert_eq!(third.renderer, Renderer::Software);
    }

    #[test]
    fn confirmed_start_keeps_gpu_and_failure_after_confirmation_is_not_remembered() {
        let directory = tempfile::tempdir().unwrap();
        let start = plan(directory.path(), Mode::Auto, None).start.unwrap();
        start.confirm();
        assert!(!directory.path().join(RECORD).exists());
        assert!(start.failed("lost the GPU device"));
        let next = plan(directory.path(), Mode::Auto, None);
        assert_eq!(next.renderer, Renderer::Gpu);
    }

    #[test]
    fn failed_automatic_start_relaunches_and_explicit_gpu_success_clears_it() {
        let directory = tempfile::tempdir().unwrap();
        let start = plan(directory.path(), Mode::Auto, None).start.unwrap();
        assert!(start.failed("no EGL display"));
        let relaunched = plan(
            directory.path(),
            Mode::Software,
            Some("no EGL display".into()),
        );
        assert_eq!(relaunched.renderer, Renderer::Software);
        assert_eq!(relaunched.notice.as_deref(), Some(NOTICE));
        let next = plan(directory.path(), Mode::Auto, None);
        assert_eq!(next.renderer, Renderer::Software);
        assert_eq!(next.notice.as_deref(), Some(NOTICE));
        assert_eq!(read(directory.path()).unwrap().reason, "no EGL display");

        let explicit = plan(directory.path(), Mode::Gpu, None).start.unwrap();
        assert!(!explicit.failed("still broken"));
        assert_eq!(
            plan(directory.path(), Mode::Auto, None).renderer,
            Renderer::Software
        );
        plan(directory.path(), Mode::Gpu, None)
            .start
            .unwrap()
            .confirm();
        assert_eq!(
            plan(directory.path(), Mode::Auto, None).renderer,
            Renderer::Gpu
        );
    }

    #[test]
    fn records_from_other_versions_and_unreadable_records_retry_the_gpu() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(RECORD),
            r#"{"version":"0.0.0","state":"failed","reason":"old driver"}"#,
        )
        .unwrap();
        assert_eq!(
            plan(directory.path(), Mode::Auto, None).renderer,
            Renderer::Gpu
        );
        std::fs::write(directory.path().join(RECORD), "not json").unwrap();
        assert_eq!(
            plan(directory.path(), Mode::Auto, None).renderer,
            Renderer::Gpu
        );
    }
}
