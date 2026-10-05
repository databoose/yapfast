//! Renderer choice at startup and recovery from a failed GPU start.
//!
//! Talk renders with Skia Software. In a ten-person call it used about half the
//! memory of Skia GPU at the same CPU, and scrolling a long history at 2× scale
//! cost less CPU too. `--renderer gpu` opts into the GPU; if the GPU fails, Talk
//! restarts itself with Skia Software and says why.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    Gpu,
    Software,
}

/// The renderer for this process, with the reason for a fallback from the GPU.
pub struct Plan {
    pub renderer: Renderer,
    pub notice: Option<String>,
}

/// `relaunch_reason` is set when this process replaced a failed GPU process.
pub fn plan(renderer: Renderer, relaunch_reason: Option<String>) -> Plan {
    let notice = match renderer {
        Renderer::Software => relaunch_reason.map(|_| NOTICE.into()),
        Renderer::Gpu => None,
    };
    Plan { renderer, notice }
}

/// The technical reason stays in the log; the notice is for people.
const NOTICE: &str = "GPU rendering failed to start, so Talk is using software rendering.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_restart_after_a_gpu_failure_shows_the_notice() {
        assert!(plan(Renderer::Software, None).notice.is_none());
        assert!(plan(Renderer::Gpu, None).notice.is_none());
        let restarted = plan(Renderer::Software, Some("no EGL display".into()));
        assert_eq!(restarted.renderer, Renderer::Software);
        assert_eq!(restarted.notice.as_deref(), Some(NOTICE));
    }
}
