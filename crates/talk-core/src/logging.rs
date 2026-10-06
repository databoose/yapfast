use tracing_subscriber::{filter::LevelFilter, filter::Targets, prelude::*};

pub fn init() {
    // Several tests in one process may call this; the first subscriber stays.
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter()))
        .try_init();
}

/// Logs to standard error, for a worker whose standard output is for Talk.
pub fn init_to_stderr() {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(filter()),
        )
        .try_init();
}

// Dependency debug logs can contain tokens or message content. Keep them off.
// `TALK_LOG=debug` shows Talk's own, such as what Go Live's servers say.
fn filter() -> Targets {
    let level = if std::env::var("TALK_LOG").is_ok_and(|level| level == "debug") {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    Targets::new()
        .with_target("talk", level)
        .with_target("talk_core", level)
}
