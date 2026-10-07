//! Linking a Discord account by QR code, beside the chat backend, so a build
//! without Matrix can link before anything else runs. The login goes into the
//! OS credential store; see `talk_discord::remote_auth`.

use talk_discord::{
    remote_auth::{self, Step, Stopped},
    token,
};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::runtime::{BackgroundRuntime, BackgroundTask};

/// How linking goes, for [`crate::application::Application::linking`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// The code to show, as the address it carries.
    Code(String),
    /// Someone scanned it, by their username, and confirms on their phone.
    Scanned(String),
    /// The account is linked and its login saved.
    Linked,
    /// The code expired before anyone confirmed it.
    Expired,
    Failed(String),
}

/// Starts linking an account on `runtime`. Its progress arrives on the
/// receiver until it ends, and dropping the task stops it.
pub fn start(runtime: &BackgroundRuntime) -> (BackgroundTask<()>, UnboundedReceiver<Progress>) {
    let (sender, progress) = mpsc::unbounded_channel();
    let task = runtime.spawn(run(move |step| {
        let _ = sender.send(step);
    }));
    (task, progress)
}

/// Links an account, telling `progress` how it goes until it ends.
async fn run(progress: impl Fn(Progress)) {
    let linked = remote_auth::link(|step| {
        progress(match step {
            Step::Code(address) => Progress::Code(address),
            Step::Scanned { name, .. } => Progress::Scanned(name),
        })
    })
    .await;
    progress(match linked.and_then(|login| token::store(&login)) {
        Ok(()) => Progress::Linked,
        Err(error) => match error.downcast_ref::<Stopped>() {
            Some(Stopped::Expired) => Progress::Expired,
            Some(stopped) => Progress::Failed(stopped.to_string()),
            None => {
                tracing::warn!(error = format!("{error:#}"), "could not link Discord");
                Progress::Failed(format!("Talk couldn't link Discord: {error}"))
            }
        },
    });
}
